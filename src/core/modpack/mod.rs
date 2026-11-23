//! 整合包（modpack）解析与安装。
//!
//! 支持的格式：Modrinth `.mrpack`、CurseForge 整合包 zip、服务端整合包。

mod curseforge;
mod modrinth;
mod server;

use std::collections::HashSet;
use std::fs;
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde_json::Value;
use sha1::{Digest, Sha1};
use sha2::Sha512;
use thiserror::Error;

use mc_launcher_core::loader::common::{LoaderSpec, LoaderVersion};

use super::config::LauncherConfig;
use super::install;
use super::i18n::{t, tf};

/// 同时下载的文件数。
const DOWNLOAD_CONCURRENCY: usize = 8;
/// 同时请求项目元数据的数量。
const METADATA_CONCURRENCY: usize = 8;
/// 单个文件的最大尝试次数（含首次）。
const DOWNLOAD_ATTEMPTS: usize = 3;
/// 进度回调的最小间隔，避免每读一块就刷新界面。
const PROGRESS_INTERVAL: Duration = Duration::from_millis(120);

/// 访问 Modrinth / CurseForge 时使用的 User-Agent。
pub(crate) const USER_AGENT: &str = concat!("lyys_mc_launcher/", env!("CARGO_PKG_VERSION"));

/// 整个进程共用同一个 HTTP 客户端，复用连接池。
pub(crate) fn http_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .connect_timeout(Duration::from_secs(15))
            .build()
            .unwrap_or_default()
    })
}

/// 整合包来源，安装后记录在实例目录里。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    Modrinth,
    CurseForge,
    Server,
}

impl Provider {
    pub fn name(self) -> &'static str {
        match self {
            Self::Modrinth => "modrinth",
            Self::CurseForge => "curseforge",
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
#[derive(Debug, Clone, Default)]
pub struct PackProgress {
    /// `loader`、`files` 或 `overrides`。
    pub stage: &'static str,
    /// 已完成文件数（`loader` 阶段是已下载任务数）。
    pub done: usize,
    pub total: usize,
    /// 已下载字节数，随下载实时增长。
    pub bytes_done: u64,
    /// 需要下载的总字节数，清单缺少大小时会偏小。
    pub bytes_total: u64,
    /// 最近一次统计出的下载速度（字节/秒）。
    pub bytes_per_second: f64,
    /// 当前正在下载的文件路径。
    pub current: Option<String>,
}

impl PackProgress {
    /// 渲染成一行人类可读的进度文本，供 GUI 与 CLI 共用。
    pub fn describe(&self) -> String {
        match self.stage {
            "loader" => format!("{} {}", t("installing loader"), self.done),
            "files" => {
                let mut line = format!(
                    "{} {}/{} · {}/{} · {}/s",
                    t("downloading files"),
                    self.done,
                    self.total,
                    human_bytes(self.bytes_done),
                    human_bytes(self.bytes_total),
                    human_bytes(self.bytes_per_second.max(0.0) as u64),
                );
                if let Some(current) = &self.current {
                    line.push_str(" · ");
                    line.push_str(current);
                }
                line
            }
            "overrides" => t("extracting overrides").to_owned(),
            _ => String::new(),
        }
    }
}

/// 整合包安装各阶段的耗时。
#[derive(Debug, Clone, Copy, Default)]
pub struct InstallTimings {
    /// 整合包解析（识别格式 + 读取清单）。
    pub manifest: Duration,
    /// 元数据解析（CurseForge 的联网查询）。
    pub metadata: Duration,
    /// Minecraft 本体安装。
    pub minecraft: Duration,
    /// 模组加载器安装。
    pub loader: Duration,
    /// Mod 文件下载。
    pub download: Duration,
    /// Overrides 解压。
    pub overrides: Duration,
}

impl InstallTimings {
    /// 单行汇总，便于写进日志。
    pub fn summary(&self) -> String {
        [
            ("manifest parsed in {}", self.manifest),
            ("metadata resolved in {}", self.metadata),
            ("Minecraft installed in {}", self.minecraft),
            ("loader installed in {}", self.loader),
            ("files downloaded in {}", self.download),
            ("overrides extracted in {}", self.overrides),
        ]
        .into_iter()
        .map(|(key, duration)| {
            let value = format!("{:.1}s", duration.as_secs_f64());
            tf(key, &[value.as_str()])
        })
        .collect::<Vec<_>>()
        .join(" · ")
    }
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
    /// 各阶段耗时。
    pub timings: InstallTimings,
}

/// 只解析不安装，用于展示整合包信息。
pub async fn inspect(path: &Path) -> Result<PackManifest, ModpackError> {
    let detected = detect(path)?;
    let (manifest, _) = load_manifest(path, &detected).await?;
    Ok(manifest)
}

/// 安装本地整合包压缩包。
pub async fn install_file(
    path: &Path,
    options: &InstallOptions<'_>,
    reporter: Arc<dyn Fn(PackProgress) + Send + Sync>,
) -> Result<InstallReport, ModpackError> {
    let detected = detect(path)?;
    let (manifest, mut timings) = load_manifest(path, &detected).await?;
    if manifest.game_version.trim().is_empty() {
        return Err(ModpackError::Parse(t("modpack has no Minecraft version").to_owned()));
    }

    // 先安装对应版本与模组加载器，得到版本 id；实例的隔离目录就是它的游戏目录
    let minecraft_dir = options.minecraft_dir.to_path_buf();
    let java = options.java.to_path_buf();
    let game_version = manifest.game_version.trim().to_owned();
    let loader = manifest.loader.clone();
    let report = reporter.clone();
    let (version_id, minecraft_time, loader_time) = tokio::task::spawn_blocking(move || {
        let mut progress = |progress: install::InstallProgress| {
            report(PackProgress {
                stage: "loader",
                done: progress.downloaded as usize,
                ..Default::default()
            });
        };
        // 基础版本与加载器分开安装，既能分别计时，也方便各自重试
        let minecraft_started = Instant::now();
        let vanilla = install_with_retry(|| {
            install::install_with_reporter(&minecraft_dir, &game_version, &mut progress)
        })?;
        let minecraft_time = minecraft_started.elapsed();

        let Some(spec) = loader else {
            return Ok::<_, ModpackError>((vanilla.version_id, minecraft_time, Duration::ZERO));
        };
        let loader_started = Instant::now();
        let installed = install_with_retry(|| {
            install::install_loader_with_reporter(
                &minecraft_dir,
                &game_version,
                spec.clone(),
                &java,
                &mut progress,
            )
        })?;
        Ok((installed.version_id, minecraft_time, loader_started.elapsed()))
    })
    .await??;
    timings.minecraft = minecraft_time;
    timings.loader = loader_time;

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
    let download_started = Instant::now();
    download_files(&files, reporter.clone()).await?;
    timings.download = download_started.elapsed();

    let overrides_path = path.to_path_buf();
    let overrides_prefix = detected.prefix.clone();
    let overrides_dirs = manifest.overrides.clone();
    let overrides_game_dir = game_dir.clone();
    let report = reporter.clone();
    let (overrides, overrides_time) = tokio::task::spawn_blocking(move || {
        report(PackProgress {
            stage: "overrides",
            ..Default::default()
        });
        let started = Instant::now();
        let extracted = extract_overrides(
            &overrides_path,
            &overrides_prefix,
            &overrides_dirs,
            &overrides_game_dir,
        )?;
        Ok::<_, ModpackError>((extracted, started.elapsed()))
    })
    .await??;
    timings.overrides = overrides_time;

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
        timings,
    })
}

/// 网络错误时重试几次，已下载完的文件会在下一轮被跳过。
fn install_with_retry<F>(
    mut install: F,
) -> Result<mc_launcher_core::install::InstallResult, ModpackError>
where
    F: FnMut() -> Result<mc_launcher_core::install::InstallResult, mc_launcher_core::LauncherError>,
{
    let mut attempt = 0;
    loop {
        attempt += 1;
        match install() {
            Ok(result) => return Ok(result),
            Err(mc_launcher_core::LauncherError::Network { .. }) if attempt < 3 => {
                std::thread::sleep(Duration::from_secs(3));
            }
            Err(error) => return Err(error.into()),
        }
    }
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
        }
    }

    Err(ModpackError::Parse(t(
        "this archive is not a supported modpack",
    )
    .to_owned()))
}

/// 读取并解析清单；顺带记录“解析清单”和“解析元数据”两段耗时。
async fn load_manifest(
    path: &Path,
    detected: &Detected,
) -> Result<(PackManifest, InstallTimings), ModpackError> {
    let manifest_started = Instant::now();
    let text = read_manifest_text(path, &detected.prefix, &detected.manifest_name)?;
    let json: Value = serde_json::from_str(text.trim_start_matches('\u{feff}'))
        .map_err(|error| ModpackError::Parse(format!("failed to parse modpack manifest: {error}")))?;
    let manifest_time = manifest_started.elapsed();

    let metadata_started = Instant::now();
    let mut manifest = parse_manifest(&json, detected.provider).await?;
    manifest.manifest_name = detected.manifest_name.clone();
    let timings = InstallTimings {
        manifest: manifest_time,
        metadata: metadata_started.elapsed(),
        ..Default::default()
    };
    Ok((manifest, timings))
}

async fn parse_manifest(
    json: &Value,
    provider: Provider,
) -> Result<PackManifest, ModpackError> {
    match provider {
        Provider::Modrinth => modrinth::parse(json),
        Provider::CurseForge => curseforge::parse(json).await,
        Provider::Server => server::parse(json),
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
    if files.is_empty() {
        return Ok(());
    }
    let client = http_client();
    let bytes_total = files.iter().filter_map(|(file, _)| file.size).sum();
    let tracker = Arc::new(Tracker::new(reporter, files.len(), bytes_total));
    let mut set = tokio::task::JoinSet::new();
    let mut next = 0;
    // 单个文件失败时继续跑完其余任务，最后统一报错
    let mut failures: Vec<String> = Vec::new();
    while next < files.len() || !set.is_empty() {
        while next < files.len() && set.len() < DOWNLOAD_CONCURRENCY {
            let (file, dest) = files[next].clone();
            next += 1;
            let tracker = Arc::clone(&tracker);
            set.spawn(async move { download_one(client, &file, &dest, &tracker).await });
        }
        if let Some(result) = set.join_next().await {
            match result {
                Ok(Ok(())) => {}
                Ok(Err(error)) => failures.push(error.to_string()),
                Err(error) => failures.push(error.to_string()),
            }
        }
    }
    tracker.emit(true);
    if failures.is_empty() {
        return Ok(());
    }
    let shown = failures.iter().take(5).cloned().collect::<Vec<_>>().join("; ");
    let more = failures.len().saturating_sub(5);
    let suffix = if more > 0 {
        format!(" (+{more} more)")
    } else {
        String::new()
    };
    Err(ModpackError::Download(format!(
        "{} of {} files failed: {shown}{suffix}",
        failures.len(),
        files.len(),
    )))
}

/// 下载过程中的字节统计，供回调按一定频率汇报。
struct Tracker {
    reporter: Arc<dyn Fn(PackProgress) + Send + Sync>,
    files_total: usize,
    files_done: AtomicUsize,
    bytes_total: u64,
    bytes_done: AtomicU64,
    current: Mutex<Option<String>>,
    rate: Mutex<Rate>,
}

struct Rate {
    last_emit: Instant,
    last_bytes: u64,
}

impl Tracker {
    fn new(
        reporter: Arc<dyn Fn(PackProgress) + Send + Sync>,
        files_total: usize,
        bytes_total: u64,
    ) -> Self {
        Self {
            reporter,
            files_total,
            files_done: AtomicUsize::new(0),
            bytes_total,
            bytes_done: AtomicU64::new(0),
            current: Mutex::new(None),
            rate: Mutex::new(Rate {
                last_emit: Instant::now(),
                last_bytes: 0,
            }),
        }
    }

    /// 开始下载某个文件，立刻汇报一次让界面切到新文件。
    fn begin(&self, path: &str) {
        if let Ok(mut current) = self.current.lock() {
            *current = Some(path.to_owned());
        }
        self.emit(true);
    }

    /// 收到一块数据后累计字节数。
    fn add_bytes(&self, bytes: u64) {
        self.bytes_done.fetch_add(bytes, Ordering::Relaxed);
        self.emit(false);
    }

    /// 文件已就绪（命中缓存或被跳过）。
    fn skip(&self, bytes: u64) {
        self.bytes_done.fetch_add(bytes, Ordering::Relaxed);
        self.files_done.fetch_add(1, Ordering::Relaxed);
        self.emit(false);
    }

    fn finish(&self) {
        self.files_done.fetch_add(1, Ordering::Relaxed);
        self.emit(false);
    }

    fn emit(&self, force: bool) {
        let bytes_done = self.bytes_done.load(Ordering::Relaxed);
        let Ok(mut rate) = self.rate.lock() else {
            return;
        };
        let elapsed = rate.last_emit.elapsed();
        if !force && elapsed < PROGRESS_INTERVAL {
            return;
        }
        let seconds = elapsed.as_secs_f64();
        let speed = if seconds > 0.0 {
            bytes_done.saturating_sub(rate.last_bytes) as f64 / seconds
        } else {
            0.0
        };
        rate.last_emit = Instant::now();
        rate.last_bytes = bytes_done;
        drop(rate);
        let current = self.current.lock().ok().and_then(|current| current.clone());
        (self.reporter)(PackProgress {
            stage: "files",
            done: self.files_done.load(Ordering::Relaxed),
            total: self.files_total,
            bytes_done,
            bytes_total: self.bytes_total,
            bytes_per_second: speed,
            current,
        });
    }
}

/// 单个文件下载失败的原因，决定是重试还是换下一个 URL。
enum FetchError {
    /// 网络抖动、5xx、429 等临时问题，可以重试。
    Retry(String),
    /// 4xx 或校验失败，重试没有意义。
    Fatal(String),
}

async fn download_one(
    client: &reqwest::Client,
    file: &PackFile,
    dest: &Path,
    tracker: &Tracker,
) -> Result<(), ModpackError> {
    if file_matches(dest, file) {
        tracker.skip(file.size.unwrap_or(0));
        return Ok(());
    }
    let mut last_error = None;
    for url in &file.urls {
        for attempt in 1..=DOWNLOAD_ATTEMPTS {
            match stream_to_file(client, url, file, dest, tracker).await {
                Ok(()) => {
                    tracker.finish();
                    return Ok(());
                }
                Err(FetchError::Retry(message)) if attempt < DOWNLOAD_ATTEMPTS => {
                    last_error = Some(message);
                    tokio::time::sleep(Duration::from_millis(500 * attempt as u64)).await;
                }
                Err(FetchError::Retry(message)) | Err(FetchError::Fatal(message)) => {
                    last_error = Some(message);
                    break;
                }
            }
        }
    }
    Err(ModpackError::Download(
        last_error.unwrap_or_else(|| format!("no download URL for {}", file.path)),
    ))
}

/// 流式下载到 `.part` 文件，边写边算校验值，成功后原子改名。
async fn stream_to_file(
    client: &reqwest::Client,
    url: &str,
    file: &PackFile,
    dest: &Path,
    tracker: &Tracker,
) -> Result<(), FetchError> {
    let mut response = client
        .get(url)
        .send()
        .await
        .map_err(|error| FetchError::Retry(format!("{url}: {error}")))?;
    let status = response.status();
    if !status.is_success() {
        let message = format!("{url} returned {status}");
        return Err(if status.is_server_error() || status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            FetchError::Retry(message)
        } else {
            FetchError::Fatal(message)
        });
    }
    if let (Some(expected), Some(length)) = (file.size, response.content_length())
        && length != expected
    {
        return Err(FetchError::Retry(format!(
            "{url} reported {length} bytes, expected {expected}"
        )));
    }
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).map_err(|error| FetchError::Fatal(error.to_string()))?;
    }

    tracker.begin(&file.path);
    let part = part_path(dest);
    match write_stream(&mut response, &part, file, tracker).await {
        Ok(()) => {}
        Err(error) => {
            let _ = fs::remove_file(&part);
            return Err(error);
        }
    }
    fs::rename(&part, dest).map_err(|error| {
        let _ = fs::remove_file(&part);
        FetchError::Fatal(format!("failed to move {} into place: {error}", file.path))
    })
}

async fn write_stream(
    response: &mut reqwest::Response,
    part: &Path,
    file: &PackFile,
    tracker: &Tracker,
) -> Result<(), FetchError> {
    let mut out = fs::File::create(part)
        .map_err(|error| FetchError::Fatal(format!("failed to create {}: {error}", part.display())))?;
    let mut sha1 = Sha1::new();
    let mut sha512 = Sha512::new();
    let mut written = 0u64;
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| FetchError::Retry(format!("{}: {error}", file.path)))?
    {
        out.write_all(&chunk)
            .map_err(|error| FetchError::Fatal(format!("failed to write {}: {error}", file.path)))?;
        sha1.update(&chunk);
        sha512.update(&chunk);
        written += chunk.len() as u64;
        tracker.add_bytes(chunk.len() as u64);
    }
    out.flush()
        .map_err(|error| FetchError::Fatal(format!("failed to flush {}: {error}", file.path)))?;
    out.sync_all()
        .map_err(|error| FetchError::Fatal(format!("failed to sync {}: {error}", file.path)))?;
    drop(out);

    let sha1_hex = hex(&sha1.finalize());
    let sha512_hex = hex(&sha512.finalize());
    if let Some(expected) = &file.sha1
        && !sha1_hex.eq_ignore_ascii_case(expected)
    {
        return Err(FetchError::Fatal(format!("sha1 mismatch for {}", file.path)));
    }
    if let Some(expected) = &file.sha512
        && !sha512_hex.eq_ignore_ascii_case(expected)
    {
        return Err(FetchError::Fatal(format!("sha512 mismatch for {}", file.path)));
    }
    if let Some(size) = file.size
        && written != size
    {
        return Err(FetchError::Retry(format!(
            "{} was {written} bytes, expected {size}",
            file.path
        )));
    }
    Ok(())
}

/// `.part` 临时文件与目标文件同目录，保证改名是同文件系统的原子操作。
fn part_path(dest: &Path) -> PathBuf {
    let mut name = dest.as_os_str().to_os_string();
    name.push(".part");
    PathBuf::from(name)
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
    if file.sha1.is_none() && file.sha512.is_none() {
        return file.size.is_some();
    }
    let Some((sha1, sha512)) = hash_file(dest) else {
        return false;
    };
    if let Some(expected) = &file.sha512 {
        return sha512.eq_ignore_ascii_case(expected);
    }
    if let Some(expected) = &file.sha1 {
        return sha1.eq_ignore_ascii_case(expected);
    }
    true
}

/// 分块读取已有文件并计算校验值，避免整包读进内存。
fn hash_file(path: &Path) -> Option<(String, String)> {
    let mut file = fs::File::open(path).ok()?;
    let mut sha1 = Sha1::new();
    let mut sha512 = Sha512::new();
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer).ok()?;
        if read == 0 {
            break;
        }
        sha1.update(&buffer[..read]);
        sha512.update(&buffer[..read]);
    }
    Some((hex(&sha1.finalize()), hex(&sha512.finalize())))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// 把字节数格式化成 1.2 MiB 这种可读形式。
fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
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
