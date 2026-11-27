use std::collections::{BTreeMap, HashSet};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Condvar, Mutex, OnceLock};
use std::time::Duration;

use mc_launcher_core::compatibility::{CompatibilityPolicy, apply_compatibility};
use mc_launcher_core::core::version::VersionJson;
use mc_launcher_core::install::assets::{
    AssetIndexJson, asset_index_path, plan_asset_index_download,
    plan_asset_object_downloads_from_index,
};
use mc_launcher_core::install::client::{version_json_path, write_version_json};
use mc_launcher_core::install::loader::{
    InstallerInvocation, run_loader_installer, write_loader_profile,
};
use mc_launcher_core::install::natives::extract_natives_for_platform;
use mc_launcher_core::install::vanilla::plan_vanilla_downloads_for_platform;
use mc_launcher_core::install::InstallResult;
use mc_launcher_core::launcher::Launcher;
use mc_launcher_core::loader::common::{LoaderSpec, LoaderVersion};
use mc_launcher_core::loader::{LoaderKind, fabric, forge, neoforge, quilt};
use mc_launcher_core::io::hash::sha1_file;
use mc_launcher_core::net::download::{Checksum, DownloadPlan, DownloadTask, should_skip_existing};
use mc_launcher_core::net::http;
use mc_launcher_core::platform::Platform;
use mc_launcher_core::progress::{ProgressEvent, SkipReason};
use serde::{Deserialize, Serialize};

use super::i18n::tf;

const VERSION_MANIFEST_URL: &str =
    "https://launchermeta.mojang.com/mc/game/version_manifest_v2.json";
const REQUEST_TIMEOUT_SECONDS: u64 = 30;
const FABRIC_GAME_LOADERS_URL: &str = "https://meta.fabricmc.net/v2/versions/loader";
const QUILT_GAME_LOADERS_URL: &str = "https://meta.quiltmc.org/v3/versions/loader";
const FORGE_METADATA_URL: &str =
    "https://maven.minecraftforge.net/net/minecraftforge/forge/maven-metadata.xml";
const NEOFORGE_METADATA_URL: &str =
    "https://maven.neoforged.net/releases/net/neoforged/neoforge/maven-metadata.xml";
const VERSIONS_CACHE: &str = "versions.json";
const LOADER_VERSIONS_CACHE: &str = "loader_versions.json";

/// 同一目录下的并发任务会下载重叠的资产与库文件，而依赖库直接把数据写到
/// 目标路径，因此按路径加互斥锁，避免两个任务同时写同一个文件导致损坏。
static PATH_LOCKS: OnceLock<(Mutex<HashSet<PathBuf>>, Condvar)> = OnceLock::new();

/// 官方安装器会改写 launcher_profiles.json 等共享档案，同一时刻只允许运行一个。
static LOADER_INSTALLER_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

struct PathGuard {
    path: PathBuf,
}

impl Drop for PathGuard {
    fn drop(&mut self) {
        let (held, cond) = PATH_LOCKS.get_or_init(Default::default);
        let mut guard = held.lock().unwrap_or_else(|error| error.into_inner());
        guard.remove(&self.path);
        cond.notify_all();
    }
}

fn path_lock(path: &Path) -> PathGuard {
    let (held, cond) = PATH_LOCKS.get_or_init(Default::default);
    let mut guard = held.lock().unwrap_or_else(|error| error.into_inner());
    while !guard.insert(path.to_path_buf()) {
        guard = cond.wait(guard).unwrap_or_else(|error| error.into_inner());
    }
    PathGuard {
        path: path.to_path_buf(),
    }
}

fn loader_installer_lock() -> std::sync::MutexGuard<'static, ()> {
    LOADER_INSTALLER_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|error| error.into_inner())
}

/// 与 mc_launcher_core 的 `execute_plan` 逻辑一致，但每个目标文件先取路径锁，
/// 使多个任务可以安全地并发安装到同一个目录。
fn execute_plan_locked(
    plan: &DownloadPlan,
    reporter: &mut dyn FnMut(ProgressEvent),
) -> Result<(), mc_launcher_core::LauncherError> {
    let client = http::client()?;
    for task in &plan.tasks {
        let result = {
            let _guard = path_lock(&task.destination);
            if should_skip_existing(task)? {
                reporter(ProgressEvent::TaskSkipped {
                    label: task.label.clone(),
                    reason: if task.checksum.is_some() {
                        SkipReason::ChecksumMatched
                    } else {
                        SkipReason::FileExistsWithoutChecksum
                    },
                });
                Ok(())
            } else {
                reporter(ProgressEvent::TaskStarted {
                    label: task.label.clone(),
                    path: task.destination.clone(),
                });
                let download = (|| -> Result<(), mc_launcher_core::LauncherError> {
                    if let Some(parent) = task.destination.parent() {
                        std::fs::create_dir_all(parent)?;
                    }
                    let mut response = client.get(&task.url).send()?.error_for_status()?;
                    let mut file = std::fs::File::create(&task.destination)?;
                    io::copy(&mut response, &mut file)?;
                    if let Some(Checksum::Sha1(expected)) = &task.checksum {
                        let actual = sha1_file(&task.destination)?;
                        if actual != *expected {
                            return Err(mc_launcher_core::LauncherError::ChecksumMismatch {
                                path: task.destination.clone(),
                                expected: expected.clone(),
                                actual,
                            });
                        }
                    }
                    Ok(())
                })();
                download.map(|_| {
                    reporter(ProgressEvent::TaskFinished {
                        label: task.label.clone(),
                    });
                })
            }
        };
        result?;
    }
    Ok(())
}

/// 官方下载域名到镜像路径的对应关系，遵循 BMCLAPI 的路径约定。
const MIRROR_MAPPINGS: &[(&str, &str)] = &[
    ("https://launchermeta.mojang.com", ""),
    ("https://piston-meta.mojang.com", ""),
    ("https://piston-data.mojang.com", ""),
    ("https://launcher.mojang.com", ""),
    ("https://resources.download.minecraft.net", "/assets"),
    ("https://libraries.minecraft.net", "/maven"),
    ("https://maven.fabricmc.net", "/maven"),
    ("https://meta.fabricmc.net", "/fabric-meta"),
    ("https://meta.quiltmc.org", "/quilt-meta"),
    ("https://maven.minecraftforge.net", "/maven"),
    ("https://maven.neoforged.net/releases", "/maven"),
];

/// 把官方下载地址替换成镜像地址；`mirror` 为空或地址已在镜像上时保持原样。
fn mirror_url(url: &str, mirror: &str) -> String {
    let mirror = mirror.trim().trim_end_matches('/');
    if mirror.is_empty() || url.starts_with(mirror) {
        return url.to_owned();
    }
    for &(origin, suffix) in MIRROR_MAPPINGS {
        if let Some(rest) = url.strip_prefix(origin) {
            return format!("{mirror}{suffix}{rest}");
        }
    }
    url.to_owned()
}

fn mirrored_plan(mut plan: DownloadPlan, mirror: &str) -> DownloadPlan {
    for task in &mut plan.tasks {
        task.url = mirror_url(&task.url, mirror);
    }
    plan
}

#[derive(Deserialize)]
struct GameLoaderEntry {
    loader: GameLoaderVersion,
}

#[derive(Deserialize)]
struct GameLoaderVersion {
    version: String,
}

/// 远端版本清单里的一条记录。
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AvailableVersion {
    pub id: String,
    pub kind: String,
}

#[derive(Deserialize)]
struct VersionManifest {
    versions: Vec<ManifestEntry>,
}

#[derive(Deserialize)]
struct ManifestEntry {
    id: String,
    #[serde(rename = "type")]
    kind: String,
    url: String,
}

/// 从 Mojang 或镜像拉取版本清单，顺序为新版本在前。
pub async fn available_versions(mirror: &str) -> Result<Vec<AvailableVersion>, reqwest::Error> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(REQUEST_TIMEOUT_SECONDS))
        .build()?;
    let manifest: VersionManifest = client
        .get(mirror_url(VERSION_MANIFEST_URL, mirror))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    Ok(manifest
        .versions
        .into_iter()
        .map(|entry| AvailableVersion {
            id: entry.id,
            kind: entry.kind,
        })
        .collect())
}

/// 上次成功拉取并缓存的版本清单。
pub fn cached_versions() -> Option<Vec<AvailableVersion>> {
    read_cache(VERSIONS_CACHE)
}

/// 记住刚拉取到的版本清单，供下次启动直接使用。
pub fn store_versions(versions: &[AvailableVersion]) {
    write_cache(VERSIONS_CACHE, versions);
}

/// 某个加载器在某个游戏版本下缓存的版本列表。
pub fn cached_loader_versions(kind: LoaderKind, minecraft_version: &str) -> Option<Vec<String>> {
    let cache: BTreeMap<String, Vec<String>> = read_cache(LOADER_VERSIONS_CACHE)?;
    cache
        .get(&loader_cache_key(kind, minecraft_version))
        .cloned()
}

/// 记住刚拉取到的加载器版本列表；空列表通常代表网络或接口异常，不缓存。
pub fn store_loader_versions(kind: LoaderKind, minecraft_version: &str, versions: &[String]) {
    if versions.is_empty() {
        return;
    }
    let mut cache: BTreeMap<String, Vec<String>> =
        read_cache(LOADER_VERSIONS_CACHE).unwrap_or_default();
    cache.insert(
        loader_cache_key(kind, minecraft_version),
        versions.to_vec(),
    );
    write_cache(LOADER_VERSIONS_CACHE, &cache);
}

fn loader_cache_key(kind: LoaderKind, minecraft_version: &str) -> String {
    format!("{kind:?}/{minecraft_version}").to_lowercase()
}

fn read_cache<T: serde::de::DeserializeOwned>(name: &str) -> Option<T> {
    let path = super::config::cache_dir().ok()?.join(name);
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

fn write_cache<T: Serialize + ?Sized>(name: &str, value: &T) {
    let Ok(dir) = super::config::cache_dir() else {
        return;
    };
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    if let Ok(bytes) = serde_json::to_vec(value) {
        let _ = std::fs::write(dir.join(name), bytes);
    }
}

/// 列出本地 `versions` 目录下已安装的实例（存在同名版本 JSON 才算）。
pub fn installed_versions(minecraft_dir: &Path) -> std::io::Result<Vec<String>> {
    let versions_dir = minecraft_dir.join("versions");
    if !versions_dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut versions = Vec::new();
    for entry in std::fs::read_dir(versions_dir)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let file_name = entry.file_name();
        let Some(name) = file_name.to_str() else {
            continue;
        };
        if version_json_path(minecraft_dir, name).is_file() {
            versions.push(name.to_owned());
        }
    }
    versions.sort();
    Ok(versions)
}

/// 安装过程中给调用方的精简进度。
#[derive(Clone, Debug)]
pub struct InstallProgress {
    pub downloaded: u64,
    pub skipped: u64,
    pub current: Option<String>,
}

pub fn install(
    minecraft_dir: &Path,
    version_id: &str,
    mirror: &str,
) -> Result<InstallResult, mc_launcher_core::LauncherError> {
    let mut downloaded = 0_u64;
    let mut skipped = 0_u64;
    let mut shown = false;
    let result = run_install(minecraft_dir, version_id, None, mirror, |event: ProgressEvent| {
        match event {
            ProgressEvent::TaskFinished { .. } => downloaded += 1,
            ProgressEvent::TaskSkipped { .. } => skipped += 1,
            _ => return,
        }
        shown = true;
        let downloaded = downloaded.to_string();
        let skipped = skipped.to_string();
        print!(
            "\r{}",
            tf(
                "install.summary",
                &[("downloaded", downloaded.as_str()), ("skipped", skipped.as_str())],
            )
        );
        let _ = std::io::stdout().flush();
    });
    if shown {
        println!();
    }
    result
}

/// 与 [`install`] 相同，但把进度回调出去，供图形界面实时展示。
pub fn install_with_reporter(
    minecraft_dir: &Path,
    version_id: &str,
    mirror: &str,
    reporter: &mut dyn FnMut(InstallProgress),
) -> Result<InstallResult, mc_launcher_core::LauncherError> {
    install_reporting(minecraft_dir, version_id, None, mirror, reporter)
}

/// 与 [`install_with_reporter`] 相同，但可以同时安装指定的模组加载器。
///
/// Forge/NeoForge 需要执行官方安装器，`java_executable` 指定使用的 Java。
pub fn install_loader_with_reporter(
    minecraft_dir: &Path,
    version_id: &str,
    loader: LoaderSpec,
    java_executable: &Path,
    mirror: &str,
    reporter: &mut dyn FnMut(InstallProgress),
) -> Result<InstallResult, mc_launcher_core::LauncherError> {
    match loader {
        LoaderSpec::Forge { .. } | LoaderSpec::NeoForge { .. } => install_with_installer(
            minecraft_dir,
            version_id,
            loader,
            java_executable,
            mirror,
            reporter,
        ),
        loader => install_reporting(minecraft_dir, version_id, Some(loader), mirror, reporter),
    }
}

/// 安装器会在当前工作目录写 `<jar 文件名>.log`，统一挪到数据目录的 `log/` 下。
fn archive_installer_log(installer_path: &Path) {
    let Some(name) = installer_path.file_name().and_then(|name| name.to_str()) else {
        return;
    };
    let log_name = format!("{name}.log");
    let source = Path::new(&log_name);
    if !source.is_file() {
        return;
    }
    let Ok(dir) = super::config::log_dir() else {
        return;
    };
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let destination = dir.join(&log_name);
    let _ = std::fs::rename(source, destination);
}

/// Forge / NeoForge 官方安装器要求目标目录里已有启动器档案文件，缺失时补一个空的。
fn ensure_launcher_profiles(minecraft_dir: &Path) {
    let path = minecraft_dir.join("launcher_profiles.json");
    if path.is_file() {
        return;
    }
    let _guard = path_lock(&path);
    if path.is_file() {
        return;
    }
    let _ = std::fs::create_dir_all(minecraft_dir);
    let _ = std::fs::write(path, br#"{"profiles":{}}"#);
}

/// 拉取某个加载器的可选版本；Forge/NeoForge 只保留匹配当前 Minecraft 版本的条目。
pub fn loader_versions(
    kind: LoaderKind,
    minecraft_version: &str,
    mirror: &str,
) -> Result<Vec<String>, mc_launcher_core::LauncherError> {
    match kind {
        LoaderKind::Fabric => {
            if minecraft_version.trim().is_empty() {
                let versions: Vec<fabric::FabricLoaderVersion> =
                    http::get_json(&mirror_url(FABRIC_GAME_LOADERS_URL, mirror))?;
                return Ok(versions.into_iter().map(|version| version.version).collect());
            }
            let entries: Vec<GameLoaderEntry> = http::get_json(&mirror_url(
                &format!("{FABRIC_GAME_LOADERS_URL}/{minecraft_version}"),
                mirror,
            ))?;
            Ok(entries
                .into_iter()
                .map(|entry| entry.loader.version)
                .collect())
        }
        LoaderKind::Quilt => {
            if minecraft_version.trim().is_empty() {
                let versions: Vec<quilt::QuiltLoaderVersion> =
                    http::get_json(&mirror_url(QUILT_GAME_LOADERS_URL, mirror))?;
                return Ok(versions.into_iter().map(|version| version.version).collect());
            }
            let entries: Vec<GameLoaderEntry> = http::get_json(&mirror_url(
                &format!("{QUILT_GAME_LOADERS_URL}/{minecraft_version}"),
                mirror,
            ))?;
            Ok(entries
                .into_iter()
                .map(|entry| entry.loader.version)
                .collect())
        }
        LoaderKind::Forge => {
            let prefix = format!("{minecraft_version}-");
            let mut versions: Vec<String> = forge_versions(mirror)?
                .into_iter()
                .filter(|version| version.starts_with(&prefix))
                .collect();
            versions.reverse();
            Ok(versions)
        }
        LoaderKind::NeoForge => {
            let prefix = neoforge_version_prefix(minecraft_version);
            let mut versions: Vec<String> = match prefix {
                Some(prefix) => neoforge_versions(mirror)?
                    .into_iter()
                    .filter(|version| version.starts_with(&prefix))
                    .collect(),
                None => Vec::new(),
            };
            versions.reverse();
            Ok(versions)
        }
    }
}

fn forge_versions(mirror: &str) -> Result<Vec<String>, mc_launcher_core::LauncherError> {
    let xml = http::get_text(&mirror_url(FORGE_METADATA_URL, mirror))?;
    Ok(forge::parse_maven_metadata(&xml)?.versions)
}

fn neoforge_versions(mirror: &str) -> Result<Vec<String>, mc_launcher_core::LauncherError> {
    let xml = http::get_text(&mirror_url(NEOFORGE_METADATA_URL, mirror))?;
    Ok(neoforge::parse_maven_metadata(&xml)?.versions)
}

fn install_with_installer(
    minecraft_dir: &Path,
    minecraft_version: &str,
    loader: LoaderSpec,
    java_executable: &Path,
    mirror: &str,
    reporter: &mut dyn FnMut(InstallProgress),
) -> Result<InstallResult, mc_launcher_core::LauncherError> {
    ensure_java_available(java_executable)?;
    ensure_launcher_profiles(minecraft_dir);
    let mut downloaded = 0_u64;
    let mut skipped = 0_u64;
    let mut current: Option<String> = None;
    let mut reporter_bridge = |event: ProgressEvent| {
        match event {
            ProgressEvent::TaskStarted { label, .. } => current = Some(label),
            ProgressEvent::TaskFinished { .. } => {
                downloaded += 1;
                current = None;
            }
            ProgressEvent::TaskSkipped { .. } => skipped += 1,
            _ => return,
        }
        reporter(InstallProgress {
            downloaded,
            skipped,
            current: current.clone(),
        });
    };

    let launcher = Launcher::new(minecraft_dir);
    install_vanilla(
        minecraft_dir,
        minecraft_version,
        mirror,
        &mut reporter_bridge,
    )?;

    let (kind, loader_version) = match &loader {
        LoaderSpec::Forge { version } => (
            LoaderKind::Forge,
            resolve_installer_version(LoaderKind::Forge, minecraft_version, version, mirror)?,
        ),
        LoaderSpec::NeoForge { version } => (
            LoaderKind::NeoForge,
            resolve_installer_version(LoaderKind::NeoForge, minecraft_version, version, mirror)?,
        ),
        _ => unreachable!("only installer-based loaders reach this path"),
    };

    let loader_name = match kind {
        LoaderKind::Forge => "forge",
        LoaderKind::NeoForge => "neoforge",
        _ => unreachable!(),
    };
    let installer_url = mirror_url(
        &match kind {
            LoaderKind::Forge => forge::installer_url(&loader_version),
            LoaderKind::NeoForge => neoforge::installer_url(&loader_version),
            _ => unreachable!(),
        },
        mirror,
    );
    let installer_path = minecraft_dir
        .join("versions")
        .join(".installers")
        .join(format!("{loader_name}-{loader_version}-installer.jar"));
    execute_plan_locked(
        &DownloadPlan {
            tasks: vec![DownloadTask {
                url: installer_url,
                destination: installer_path.clone(),
                checksum: None,
                label: format!("{loader_name} installer {loader_version}"),
            }],
        },
        &mut reporter_bridge,
    )?;

    let version_id = match kind {
        LoaderKind::Forge => forge::forge_installed_version_id(&loader_version)?,
        LoaderKind::NeoForge => {
            neoforge::neoforge_installed_version_id(minecraft_version, &loader_version)
        }
        _ => unreachable!(),
    };
    let result = {
        let _installer_guard = loader_installer_lock();
        let result = run_loader_installer(&InstallerInvocation {
            loader: kind,
            java_executable: java_executable.to_path_buf(),
            installer_path: installer_path.clone(),
            minecraft_dir: minecraft_dir.to_path_buf(),
        });
        archive_installer_log(&installer_path);
        result
    };
    result?;

    {
        let _guard = path_lock(&minecraft_dir.join("versions").join(&version_id));
        let merged = launcher.load_version(&version_id)?;
        install_version_files(&merged, minecraft_dir, mirror, &mut reporter_bridge)?;
    }
    Ok(InstallResult { version_id })
}

fn resolve_installer_version(
    kind: LoaderKind,
    minecraft_version: &str,
    version: &LoaderVersion,
    mirror: &str,
) -> Result<String, mc_launcher_core::LauncherError> {
    match version {
        LoaderVersion::Exact(version) => Ok(match kind {
            LoaderKind::Forge => normalize_forge_version(minecraft_version, version),
            _ => version.clone(),
        }),
        LoaderVersion::Latest | LoaderVersion::LatestStable => match kind {
            LoaderKind::Forge => Ok(
                forge::latest_for_minecraft(&forge_versions(mirror)?, minecraft_version)?.to_owned(),
            ),
            LoaderKind::NeoForge => Ok(
                neoforge::latest_for_minecraft(&neoforge_versions(mirror)?, minecraft_version)?
                    .to_owned(),
            ),
            _ => unreachable!(),
        },
    }
}

fn ensure_java_available(java_executable: &Path) -> Result<(), mc_launcher_core::LauncherError> {
    let failure = |message: String| mc_launcher_core::LauncherError::Io {
        source: std::io::Error::other(message),
    };
    let output = std::process::Command::new(java_executable)
        .arg("-version")
        .output()
        .map_err(|error| {
            failure(format!(
                "cannot run Java executable `{}`: {error}; set Java to a valid java executable",
                java_executable.display()
            ))
        })?;
    if output.status.success() {
        Ok(())
    } else {
        Err(failure(format!(
            "`{} -version` failed; set Java to a valid java executable",
            java_executable.display()
        )))
    }
}

fn normalize_forge_version(minecraft_version: &str, version: &str) -> String {
    if version.contains('-') || minecraft_version.trim().is_empty() {
        version.to_owned()
    } else {
        format!("{minecraft_version}-{version}")
    }
}

/// 与 mc-launcher-core 的 NeoForge 版本匹配规则保持一致。
fn neoforge_version_prefix(minecraft_version: &str) -> Option<String> {
    if let Some(legacy) = minecraft_version.strip_prefix("1.") {
        let mut components = legacy.split('.');
        let release = components.next()?;
        let patch = components.next().unwrap_or("0");
        if components.next().is_some() || !numeric_components(&[release, patch]) {
            return None;
        }
        return Some(format!("{release}.{patch}."));
    }

    let mut components = minecraft_version.split('.');
    let year = components.next()?;
    let release = components.next()?;
    let patch = components.next().unwrap_or("0");
    if components.next().is_some()
        || !year.parse::<u32>().is_ok_and(|year| year >= 26)
        || !numeric_components(&[year, release, patch])
    {
        return None;
    }
    Some(format!("{year}.{release}.{patch}."))
}

fn numeric_components(components: &[&str]) -> bool {
    components.iter().all(|component| {
        !component.is_empty() && component.chars().all(|character| character.is_ascii_digit())
    })
}

fn install_reporting(
    minecraft_dir: &Path,
    version_id: &str,
    loader: Option<LoaderSpec>,
    mirror: &str,
    reporter: &mut dyn FnMut(InstallProgress),
) -> Result<InstallResult, mc_launcher_core::LauncherError> {
    let mut downloaded = 0_u64;
    let mut skipped = 0_u64;
    let mut current: Option<String> = None;
    run_install(minecraft_dir, version_id, loader, mirror, |event: ProgressEvent| {
        match event {
            ProgressEvent::TaskStarted { label, .. } => current = Some(label),
            ProgressEvent::TaskFinished { .. } => {
                downloaded += 1;
                current = None;
            }
            ProgressEvent::TaskSkipped { .. } => skipped += 1,
            _ => return,
        }
        reporter(InstallProgress {
            downloaded,
            skipped,
            current: current.clone(),
        });
    })
}

fn run_install<F>(
    minecraft_dir: &Path,
    version_id: &str,
    loader: Option<LoaderSpec>,
    mirror: &str,
    mut on_event: F,
) -> Result<InstallResult, mc_launcher_core::LauncherError>
where
    F: FnMut(ProgressEvent),
{
    match loader {
        Some(loader @ (LoaderSpec::Fabric { .. } | LoaderSpec::Quilt { .. })) => {
            install_vanilla(minecraft_dir, version_id, mirror, &mut on_event)?;
            install_native_loader(minecraft_dir, version_id, &loader, mirror, &mut on_event)
        }
        Some(_) => unreachable!("installer-based loaders are handled by install_with_installer"),
        None => {
            install_vanilla(minecraft_dir, version_id, mirror, &mut on_event)?;
            Ok(InstallResult {
                version_id: version_id.to_owned(),
            })
        }
    }
}

fn install_vanilla<F>(
    minecraft_dir: &Path,
    version_id: &str,
    mirror: &str,
    reporter: &mut F,
) -> Result<(), mc_launcher_core::LauncherError>
where
    F: FnMut(ProgressEvent),
{
    // 同一版本可能被两个任务同时安装，整个阶段按版本目录串行。
    let _guard = path_lock(&minecraft_dir.join("versions").join(version_id));
    let version = fetch_version_json(version_id, mirror)?;
    write_version_json(minecraft_dir, &version)?;
    install_version_files(&version, minecraft_dir, mirror, reporter)
}

fn fetch_version_json(
    version_id: &str,
    mirror: &str,
) -> Result<VersionJson, mc_launcher_core::LauncherError> {
    let manifest: VersionManifest =
        http::get_json(&mirror_url(VERSION_MANIFEST_URL, mirror))?;
    let entry = manifest
        .versions
        .iter()
        .find(|entry| entry.id == version_id)
        .ok_or_else(|| mc_launcher_core::LauncherError::InvalidVersionId {
            id: version_id.to_owned(),
        })?;
    http::get_json(&mirror_url(&entry.url, mirror))
}

fn install_native_loader<F>(
    minecraft_dir: &Path,
    minecraft_version: &str,
    loader: &LoaderSpec,
    mirror: &str,
    reporter: &mut F,
) -> Result<InstallResult, mc_launcher_core::LauncherError>
where
    F: FnMut(ProgressEvent),
{
    let (profile, profile_name) = match loader {
        LoaderSpec::Fabric { version } => {
            let loader_version = match version {
                LoaderVersion::Exact(version) => version.clone(),
                LoaderVersion::Latest | LoaderVersion::LatestStable => {
                    let versions: Vec<fabric::FabricLoaderVersion> =
                        http::get_json(&mirror_url(FABRIC_GAME_LOADERS_URL, mirror))?;
                    fabric::latest_stable_loader(&versions)?.version.clone()
                }
            };
            let profile: VersionJson = http::get_json(&mirror_url(
                &format!(
                    "{FABRIC_GAME_LOADERS_URL}/{minecraft_version}/{loader_version}/profile/json"
                ),
                mirror,
            ))?;
            (profile, "fabric profile")
        }
        LoaderSpec::Quilt { version } => {
            let loader_version = match version {
                LoaderVersion::Exact(version) => version.clone(),
                LoaderVersion::Latest | LoaderVersion::LatestStable => {
                    let versions: Vec<quilt::QuiltLoaderVersion> =
                        http::get_json(&mirror_url(QUILT_GAME_LOADERS_URL, mirror))?;
                    quilt::latest_loader(&versions)?.version.clone()
                }
            };
            let profile: VersionJson = http::get_json(&mirror_url(
                &format!(
                    "{QUILT_GAME_LOADERS_URL}/{minecraft_version}/{loader_version}/profile/json"
                ),
                mirror,
            ))?;
            (profile, "quilt profile")
        }
        _ => unreachable!("only Fabric and Quilt use native loader profiles"),
    };
    let version_id =
        profile
            .id
            .clone()
            .ok_or_else(|| mc_launcher_core::LauncherError::MissingField {
                context: profile_name.to_owned(),
                field: "id".to_owned(),
            })?;
    let _guard = path_lock(&minecraft_dir.join("versions").join(&version_id));
    write_loader_profile(minecraft_dir, &profile)?;
    let merged = Launcher::new(minecraft_dir).load_version(&version_id)?;
    install_version_files(&merged, minecraft_dir, mirror, reporter)?;
    Ok(InstallResult { version_id })
}

fn install_version_files<F>(
    version: &VersionJson,
    minecraft_dir: &Path,
    mirror: &str,
    reporter: &mut F,
) -> Result<(), mc_launcher_core::LauncherError>
where
    F: FnMut(ProgressEvent),
{
    let platform = Platform::current();
    let compatibility = apply_compatibility(version, platform, CompatibilityPolicy::Auto);
    let version = &compatibility.version;
    let version_id =
        version
            .id
            .as_deref()
            .ok_or_else(|| mc_launcher_core::LauncherError::MissingField {
                context: "version json".to_owned(),
                field: "id".to_owned(),
            })?;

    let plan = plan_vanilla_downloads_for_platform(
        version,
        minecraft_dir,
        platform,
        CompatibilityPolicy::Disabled,
    )?;
    execute_plan_locked(&mirrored_plan(plan, mirror), reporter)?;

    let index_plan = DownloadPlan {
        tasks: plan_asset_index_download(version, minecraft_dir)?,
    };
    execute_plan_locked(&mirrored_plan(index_plan, mirror), reporter)?;

    if let Some(asset_index) = &version.asset_index {
        let index: AssetIndexJson =
            serde_json::from_slice(&std::fs::read(asset_index_path(minecraft_dir, &asset_index.id))?)?;
        let object_plan = plan_asset_object_downloads_from_index(&index, minecraft_dir);
        execute_plan_locked(&mirrored_plan(object_plan, mirror), reporter)?;
    }

    extract_natives_for_platform(&version.libraries, minecraft_dir, version_id, platform)?;
    Ok(())
}
