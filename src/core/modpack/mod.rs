//! 整合包（modpack）解析与安装。
//!
//! 支持的格式：Modrinth `.mrpack`、CurseForge 整合包 zip、MCBBS 整合包、服务端整合包。

mod curseforge;
mod mcbbs;
mod modrinth;
mod server;

use std::collections::HashSet;
use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use serde_json::Value;
use thiserror::Error;

use mc_launcher_core::loader::common::{LoaderSpec, LoaderVersion};

use super::config::LauncherConfig;
use super::install;
use super::i18n::t;

/// 同时下载的文件数。
const DOWNLOAD_CONCURRENCY: usize = 8;

/// 访问 Modrinth / CurseForge 时使用的 User-Agent。
pub(crate) const USER_AGENT: &str = concat!("lyys_mc_launcher/", env!("CARGO_PKG_VERSION"));
/// 整合包来源，安装后记录在实例目录里。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    Modrinth,
    CurseForge,
    Mcbbs,
    Server,
}

impl Provider {
    pub fn name(self) -> &'static str {
        match self {
            Self::Modrinth => "modrinth",
            Self::CurseForge => "curseforge",
            Self::Mcbbs => "mcbbs",
            Self::Server => "server",
        }
    }
}

#[derive(Debug, Error)]
pub enum ModpackError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Zip(#[from] zip::result::ZipError),
    #[error(transparent)]
    Http(#[from] reqwest::Error),
    #[error(transparent)]
    Install(#[from] mc_launcher_core::LauncherError),
    #[error("modpack download failed: {0}")]
    Download(String),
    #[error("{0}")]
    Parse(String),
    #[error("modpack task failed: {0}")]
    Task(#[from] tokio::task::JoinError),
}

/// 整合包里需要下载的一个文件。
#[derive(Debug, Clone)]
pub struct PackFile {
    pub path: String,
    pub size: Option<u64>,
    pub sha1: Option<String>,
    pub sha512: Option<String>,
    pub urls: Vec<String>,
    /// 标记为可选、只有用户勾选后才安装的文件。
    pub optional: bool,
    /// 客户端不需要的文件（例如服务端专用模组）。
    pub client_unsupported: bool,
}

/// 解析后的整合包描述。
#[derive(Debug, Clone)]
pub struct PackManifest {
    pub provider: Provider,
    pub name: String,
    pub version: String,
    pub author: String,
    pub description: String,
    pub game_version: String,
    pub loader: Option<LoaderSpec>,
    pub files: Vec<PackFile>,
    /// 需要从压缩包解压到游戏目录的覆盖文件夹（相对路径）。
    pub overrides: Vec<String>,
    pub manifest_name: String,
}

impl PackManifest {
    /// 这个整合包实际会安装的文件数。
    pub fn installable_files(&self, with_optional: bool) -> usize {
        self.files
            .iter()
            .filter(|file| !file.client_unsupported)
            .filter(|file| with_optional || !file.optional)
            .count()
    }

    pub fn installable_bytes(&self, with_optional: bool) -> u64 {
        self.files
            .iter()
            .filter(|file| !file.client_unsupported)
            .filter(|file| with_optional || !file.optional)
            .filter_map(|file| file.size)
            .sum()
    }
}

/// 压缩包里识别出的整合包格式。
pub struct Detected {
    pub provider: Provider,
    /// 压缩包内的顶层目录前缀，没有嵌套目录时为空字符串。
    pub prefix: String,
    pub manifest_name: String,
}

/// 安装进度：文件下载与覆盖解压。
#[derive(Debug, Clone)]
pub struct PackProgress {
    /// `loader`、`files` 或 `overrides`。
    pub stage: &'static str,
    pub done: usize,
    pub total: usize,
}

pub struct InstallOptions<'a> {
    pub minecraft_dir: &'a Path,
    /// 运行 Forge/NeoForge 安装器时使用的 Java。
    pub java: &'a Path,
    pub with_optional: bool,
}

#[derive(Debug, Clone)]
pub struct InstallReport {
    pub version_id: String,
    pub game_dir: PathBuf,
    pub files: usize,
    pub overrides: usize,
    pub excluded: Vec<String>,
}

/// 只解析不安装，用于展示整合包信息。
pub async fn inspect(path: &Path) -> Result<PackManifest, ModpackError> {
    let detected = detect(path)?;
    let mut manifest = read_manifest(path, &detected).await?;
    manifest.manifest_name = detected.manifest_name.clone();
    Ok(manifest)
}

/// 安装本地整合包压缩包。
pub async fn install_file(
    path: &Path,
    options: &InstallOptions<'_>,
    reporter: Arc<dyn Fn(PackProgress) + Send + Sync>,
) -> Result<InstallReport, ModpackError> {
    let detected = detect(path)?;
    let mut manifest = read_manifest(path, &detected).await?;
    manifest.manifest_name = detected.manifest_name.clone();
    if manifest.game_version.trim().is_empty() {
        return Err(ModpackError::Parse(t("modpack has no Minecraft version").to_owned()));
    }

    // 先安装对应版本与模组加载器，得到版本 id；实例的隔离目录就是它的游戏目录
    let minecraft_dir = options.minecraft_dir.to_path_buf();
    let java = options.java.to_path_buf();
    let game_version = manifest.game_version.trim().to_owned();
    let loader = manifest.loader.clone();
    let report = reporter.clone();
    let version_id = tokio::task::spawn_blocking(move || {
        let mut progress = |progress: install::InstallProgress| {
            report(PackProgress {
                stage: "loader",
                done: progress.downloaded as usize,
                total: 0,
            });
        };
        // 下载源偶尔会中断连接，重试几次通常能续上（已下载的文件会被跳过）
        let mut attempt = 0;
        loop {
            attempt += 1;
            let result = match &loader {
                Some(spec) => install::install_loader_with_reporter(
                    &minecraft_dir,
                    &game_version,
                    spec.clone(),
                    &java,
                    &mut progress,
                ),
                None => {
                    install::install_with_reporter(&minecraft_dir, &game_version, &mut progress)
                }
            };
            match result {
                Ok(result) => break Ok(result.version_id),
                Err(mc_launcher_core::LauncherError::Network { .. }) if attempt < 3 => {
                    std::thread::sleep(std::time::Duration::from_secs(3));
                }
                Err(error) => break Err(error),
            }
        }
    })
    .await??;

    let game_dir = options.minecraft_dir.join("versions").join(&version_id);
    fs::create_dir_all(&game_dir)?;

    let mut files = Vec::new();
    let mut excluded = Vec::new();
    let mut seen = HashSet::new();
    for file in &manifest.files {
        if file.client_unsupported {
            continue;
        }
        if file.optional && !options.with_optional {
            excluded.push(file.path.clone());
            continue;
        }
        if !seen.insert(file.path.clone()) {
            return Err(ModpackError::Parse(format!(
                "modpack contains duplicate file path: {}",
                file.path
            )));
        }
        let relative = safe_relative(&file.path)?;
        files.push((file.clone(), game_dir.join(relative)));
    }

    let total = files.len();
    download_files(&files, reporter.clone()).await?;

    let overrides_path = path.to_path_buf();
    let overrides_prefix = detected.prefix.clone();
    let overrides_dirs = manifest.overrides.clone();
    let overrides_game_dir = game_dir.clone();
    let report = reporter.clone();
    let overrides = tokio::task::spawn_blocking(move || {
        report(PackProgress {
            stage: "overrides",
            done: 0,
            total: 0,
        });
        extract_overrides(
            &overrides_path,
            &overrides_prefix,
            &overrides_dirs,
            &overrides_game_dir,
        )
    })
    .await??;

    copy_entry(
        path,
        &format!("{}{}", detected.prefix, manifest.manifest_name),
        &game_dir.join(&manifest.manifest_name),
    )?;
    write_record(&game_dir, &manifest, path, &excluded)?;
    isolate_instance(&version_id)?;

    Ok(InstallReport {
        version_id,
        game_dir,
        files: total,
        overrides,
        excluded,
    })
}

/// 识别压缩包属于哪种整合包格式。
pub fn detect(path: &Path) -> Result<Detected, ModpackError> {
    let mut archive = open_archive(path)?;
    let names: Vec<String> = (0..archive.len())
        .map(|index| {
            archive
                .by_index(index)
                .ok()
                .and_then(|entry| entry.name().ok().map(|name| normalize_name(&name)))
                .unwrap_or_default()
        })
        .collect();
    let name_set: HashSet<&str> = names.iter().map(String::as_str).collect();
    let mut prefixes = vec![String::new()];
    let mut nested = std::collections::BTreeSet::new();
    for name in &names {
        if let Some((top, _)) = name.split_once('/') {
            nested.insert(format!("{top}/"));
        }
    }
    prefixes.extend(nested);

    for prefix in prefixes {
        if name_set.contains(format!("{prefix}modrinth.index.json").as_str()) {
            return Ok(Detected {
                provider: Provider::Modrinth,
                prefix,
                manifest_name: "modrinth.index.json".to_owned(),
            });
        }
        if name_set.contains(format!("{prefix}mcbbs.packmeta").as_str()) {
            return Ok(Detected {
                provider: Provider::Mcbbs,
                prefix,
                manifest_name: "mcbbs.packmeta".to_owned(),
            });
        }
        if name_set.contains(format!("{prefix}server-manifest.json").as_str()) {
            return Ok(Detected {
                provider: Provider::Server,
                prefix,
                manifest_name: "server-manifest.json".to_owned(),
            });
        }
        let manifest_name = format!("{prefix}manifest.json");
        if name_set.contains(manifest_name.as_str())
            && let Some(text) = read_entry_text(&mut archive, &manifest_name)
            && let Ok(json) = serde_json::from_str::<Value>(&text)
        {
            if json["minecraft"].is_object() {
                return Ok(Detected {
                    provider: Provider::CurseForge,
                    prefix,
                    manifest_name: "manifest.json".to_owned(),
                });
            }
            if json.get("addons").is_some() || json.get("launchInfo").is_some() {
                return Ok(Detected {
                    provider: Provider::Mcbbs,
                    prefix,
                    manifest_name: "manifest.json".to_owned(),
                });
            }
        }
    }

    Err(ModpackError::Parse(t(
        "this archive is not a supported modpack",
    )
    .to_owned()))
}

async fn read_manifest(path: &Path, detected: &Detected) -> Result<PackManifest, ModpackError> {
    let text = read_manifest_text(path, &detected.prefix, &detected.manifest_name)?;
    let json: Value = serde_json::from_str(text.trim_start_matches('\u{feff}'))
        .map_err(|error| ModpackError::Parse(format!("failed to parse modpack manifest: {error}")))?;
    match detected.provider {
        Provider::Modrinth => modrinth::parse(&json),
        Provider::CurseForge => curseforge::parse(&json).await,
        Provider::Mcbbs => mcbbs::parse(&json).await,
        Provider::Server => server::parse(&json),
    }
}

fn read_manifest_text(path: &Path, prefix: &str, name: &str) -> Result<String, ModpackError> {
    let mut archive = open_archive(path)?;
    read_entry_text(&mut archive, &format!("{prefix}{name}"))
        .ok_or_else(|| ModpackError::Parse(format!("modpack has no {name}")))
}

fn open_archive(path: &Path) -> Result<zip::ZipArchive<fs::File>, ModpackError> {
    let file = fs::File::open(path).map_err(|error| {
        ModpackError::Io(std::io::Error::new(
            error.kind(),
            format!("failed to open {}: {error}", path.display()),
        ))
    })?;
    Ok(zip::ZipArchive::new(file)?)
}

fn read_entry_text(archive: &mut zip::ZipArchive<fs::File>, name: &str) -> Option<String> {
    let mut entry = archive.by_name(name).ok()?;
    let mut text = String::new();
    entry.read_to_string(&mut text).ok()?;
    Some(text)
}

fn copy_entry(path: &Path, name: &str, dest: &Path) -> Result<bool, ModpackError> {
    let mut archive = open_archive(path)?;
    let Ok(mut entry) = archive.by_name(name) else {
        return Ok(false);
    };
    let mut bytes = Vec::new();
    entry.read_to_end(&mut bytes)?;
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(dest, bytes)?;
    Ok(true)
}

/// 把压缩包里的覆盖文件夹解压到游戏目录，返回解压的文件数。
fn extract_overrides(
    path: &Path,
    prefix: &str,
    dirs: &[String],
    game_dir: &Path,
) -> Result<usize, ModpackError> {
    let mut archive = open_archive(path)?;
    let mut extracted = 0;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index)?;
        let name = match entry.name() {
            Ok(name) => normalize_name(&name),
            Err(_) => continue,
        };
        if name.ends_with('/') {
            continue;
        }
        let Some(rest) = dirs.iter().find_map(|dir| {
            name.strip_prefix(&format!("{prefix}{dir}/"))
                .map(str::to_owned)
        }) else {
            continue;
        };
        let relative = safe_relative(&rest)?;
        let dest = game_dir.join(relative);
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut file = fs::File::create(&dest)?;
        std::io::copy(&mut entry, &mut file)?;
        extracted += 1;
    }
    Ok(extracted)
}

/// 下载整合包文件；已有且校验通过的文件会跳过。
async fn download_files(
    files: &[(PackFile, PathBuf)],
    reporter: Arc<dyn Fn(PackProgress) + Send + Sync>,
) -> Result<(), ModpackError> {
    let client = reqwest::Client::builder().user_agent(USER_AGENT).build()?;
    let total = files.len();
    let mut set = tokio::task::JoinSet::new();
    let mut next = 0;
    let mut done = 0;
    while next < total || !set.is_empty() {
        while next < total && set.len() < DOWNLOAD_CONCURRENCY {
            let (file, dest) = files[next].clone();
            next += 1;
            let client = client.clone();
            set.spawn(async move { download_one(&client, &file, &dest).await });
        }
        if let Some(result) = set.join_next().await {
            result??;
            done += 1;
            reporter(PackProgress {
                stage: "files",
                done,
                total,
            });
        }
    }
    Ok(())
}

async fn download_one(
    client: &reqwest::Client,
    file: &PackFile,
    dest: &Path,
) -> Result<(), ModpackError> {
    if file_matches(dest, file) {
        return Ok(());
    }
    let mut last_error = None;
    for url in &file.urls {
        match client.get(url).send().await {
            Ok(response) if response.status().is_success() => {
                let bytes = response.bytes().await?;
                if let Some(expected) = &file.sha1
                    && !hex_sha1(&bytes).eq_ignore_ascii_case(expected)
                {
                    last_error = Some(format!("sha1 mismatch for {}", file.path));
                    continue;
                }
                if let Some(expected) = &file.sha512
                    && !hex_sha512(&bytes).eq_ignore_ascii_case(expected)
                {
                    last_error = Some(format!("sha512 mismatch for {}", file.path));
                    continue;
                }
                if let Some(size) = file.size
                    && bytes.len() as u64 != size
                {
                    last_error = Some(format!("size mismatch for {}", file.path));
                    continue;
                }
                if let Some(parent) = dest.parent() {
                    fs::create_dir_all(parent)?;
                }
                fs::write(dest, &bytes)?;
                return Ok(());
            }
            Ok(response) => {
                last_error = Some(format!("{url} returned {}", response.status()));
            }
            Err(error) => {
                last_error = Some(error.to_string());
            }
        }
    }
    Err(ModpackError::Download(
        last_error.unwrap_or_else(|| format!("no download URL for {}", file.path)),
    ))
}

fn file_matches(dest: &Path, file: &PackFile) -> bool {
    let Ok(metadata) = fs::metadata(dest) else {
        return false;
    };
    if let Some(size) = file.size
        && metadata.len() != size
    {
        return false;
    }
    if file.sha1.is_none() && file.sha512.is_none() && file.size.is_none() {
        return false;
    }
    let Ok(bytes) = fs::read(dest) else {
        return false;
    };
    if let Some(expected) = &file.sha512 {
        return hex_sha512(&bytes).eq_ignore_ascii_case(expected);
    }
    if let Some(expected) = &file.sha1 {
        return hex_sha1(&bytes).eq_ignore_ascii_case(expected);
    }
    true
}

fn hex_sha1(bytes: &[u8]) -> String {
    use sha1::{Digest, Sha1};
    let digest = Sha1::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn hex_sha512(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha512};
    let digest = Sha512::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// 校验并规范化压缩包内的相对路径，拒绝绝对路径和 `..`。
pub(crate) fn safe_relative(path: &str) -> Result<PathBuf, ModpackError> {
    let normalized = normalize_name(path);
    if normalized.trim().is_empty() {
        return Err(ModpackError::Parse("modpack contains an empty path".to_owned()));
    }
    let mut relative = PathBuf::new();
    for component in Path::new(&normalized).components() {
        match component {
            Component::Normal(part) => relative.push(part),
            _ => {
                return Err(ModpackError::Parse(format!(
                    "modpack contains an unsafe path: {path}"
                )));
            }
        }
    }
    if relative.as_os_str().is_empty() {
        return Err(ModpackError::Parse(format!(
            "modpack contains an unsafe path: {path}"
        )));
    }
    Ok(relative)
}

/// 把路径里的反斜杠统一成正斜杠，压缩包内部一律使用 `/`。
pub(crate) fn normalize_name(name: &str) -> String {
    name.replace('\\', "/")
}

/// 对 URL 路径逐段做百分号编码，保留 `/`。
pub(crate) fn encode_path(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for byte in path.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => {
                out.push(byte as char);
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

pub(crate) fn loader_spec(kind: &str, version: &str) -> Option<LoaderSpec> {
    let version = LoaderVersion::Exact(version.to_owned());
    match kind {
        "fabric" => Some(LoaderSpec::Fabric { version }),
        "quilt" => Some(LoaderSpec::Quilt { version }),
        "forge" => Some(LoaderSpec::Forge { version }),
        "neoforge" => Some(LoaderSpec::NeoForge { version }),
        _ => None,
    }
}

pub(crate) fn describe_loader(spec: &LoaderSpec) -> String {
    let (name, version) = match spec {
        LoaderSpec::Fabric { version } => ("fabric", version),
        LoaderSpec::Quilt { version } => ("quilt", version),
        LoaderSpec::Forge { version } => ("forge", version),
        LoaderSpec::NeoForge { version } => ("neoforge", version),
    };
    match version {
        LoaderVersion::Exact(version) => format!("{name} {version}"),
        _ => name.to_owned(),
    }
}

/// 写一份整合包记录，方便以后识别这个实例的来历。
fn write_record(
    game_dir: &Path,
    manifest: &PackManifest,
    source: &Path,
    excluded: &[String],
) -> Result<(), ModpackError> {
    let record = serde_json::json!({
        "provider": manifest.provider.name(),
        "name": manifest.name,
        "version": manifest.version,
        "game_version": manifest.game_version,
        "loader": manifest.loader.as_ref().map(describe_loader),
        "source": source.display().to_string(),
        "excluded": excluded,
    });
    let bytes = serde_json::to_vec_pretty(&record)
        .map_err(|error| ModpackError::Parse(error.to_string()))?;
    fs::write(game_dir.join(".lyys_modpack.json"), bytes)?;
    Ok(())
}

/// 整合包的内容放在隔离的游戏目录里，安装后把这个版本标记为隔离实例。
fn isolate_instance(version_id: &str) -> Result<(), ModpackError> {
    let mut config = LauncherConfig::load().map_err(|error| ModpackError::Parse(error.to_string()))?;
    let entry = config.instances.entry(version_id.to_owned()).or_default();
    entry.use_global = false;
    entry.isolate = true;
    config.save().map_err(|error| ModpackError::Parse(error.to_string()))?;
    Ok(())
}

/// 打开整合包压缩包，供 CLI 预览时复用路径检查。

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_unsafe_paths() {
        assert!(safe_relative("mods/a.jar").is_ok());
        assert!(safe_relative("../escape.jar").is_err());
        assert!(safe_relative("/abs.jar").is_err());
        assert!(safe_relative("mods/../../x").is_err());
    }

    #[test]
    fn encodes_paths_segment_wise() {
        assert_eq!(encode_path("mods/a b.jar"), "mods/a%20b.jar");
        assert_eq!(encode_path("config/x+y.json"), "config/x%2By.json");
    }

    #[test]
    fn maps_loader_ids() {
        assert!(loader_spec("fabric", "0.15.0").is_some());
        assert!(loader_spec("neoforge", "20.4.100").is_some());
        assert!(loader_spec("unknown", "1").is_none());
    }
}
