use std::collections::hash_map::DefaultHasher;
use std::collections::{HashSet, VecDeque};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use serde::Deserialize;
use thiserror::Error;

use crate::core::config;
use crate::core::i18n::tf;

const SEARCH_URL: &str = "https://api.modrinth.com/v2/search";
const VERSION_URL: &str = "https://api.modrinth.com/v2/version";
const USER_AGENT: &str = concat!("lyys_mc_launcher/", env!("CARGO_PKG_VERSION"));
/// 搜索接口每页返回的条目数，GUI 用它计算总页数。
pub const PAGE_SIZE: u32 = 20;
/// 前置链异常时防止无限递归。
const MAX_INSTALLED_MODS: usize = 64;

#[derive(Debug, Error)]
pub enum ModrinthError {
    #[error("Modrinth request failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("project not found on Modrinth: {0}")]
    ProjectNotFound(String),
    #[error("no compatible version found for {0}")]
    NoCompatibleVersion(String),
    #[error("{0} has no downloadable file")]
    NoFile(String),
    #[error("failed to save the mod: {0}")]
    Io(#[from] std::io::Error),
}

impl ModrinthError {
    /// Localized text for the interface; `Display` stays the English diagnostic.
    pub fn message(&self) -> String {
        match self {
            Self::ProjectNotFound(project) => {
                tf("cli.error.project_not_found", &[("project", project.as_str())])
            }
            Self::NoCompatibleVersion(project) => {
                tf("cli.error.no_compatible_version", &[("project", project.as_str())])
            }
            Self::NoFile(project) => {
                tf("cli.error.no_file", &[("project", project.as_str())])
            }
            other => {
                let detail = other.to_string();
                tf("cli.error.modrinth", &[("error", detail.as_str())])
            }
        }
    }
}

/// 单个内容文件的下载进度，供 GUI 展示。
#[derive(Debug, Clone)]
pub struct ContentProgress {
    pub filename: String,
    pub downloaded: u64,
    /// 文件总字节数，接口未提供时为 0。
    pub total: u64,
}

impl ContentProgress {
    /// 渲染成一行人类可读的进度文本。
    pub fn describe(&self) -> String {
        let done = human_bytes(self.downloaded);
        if self.total > 0 {
            format!("{}  {done} / {}", self.filename, human_bytes(self.total))
        } else {
            format!("{}  {done}", self.filename)
        }
    }
}

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

/// 进度回调的最小间隔，避免高频重绘拖慢界面。
const CONTENT_PROGRESS_INTERVAL: std::time::Duration = std::time::Duration::from_millis(120);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectType {
    Mod,
    ResourcePack,
    ShaderPack,
    DataPack,
    Modpack,
}

impl ProjectType {
    /// Modrinth `project_type` facet value.
    pub fn facet(self) -> &'static str {
        match self {
            Self::Mod => "mod",
            Self::ResourcePack => "resourcepack",
            Self::ShaderPack => "shader",
            Self::DataPack => "datapack",
            Self::Modpack => "modpack",
        }
    }

    /// Path segment of the public project page.
    pub fn url_segment(self) -> &'static str {
        match self {
            Self::Mod => "mod",
            Self::ResourcePack => "resourcepack",
            Self::ShaderPack => "shader",
            Self::DataPack => "datapack",
            Self::Modpack => "modpack",
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct ModSearchHit {
    #[serde(default)]
    pub project_id: String,
    pub slug: String,
    pub title: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub icon_url: Option<String>,
    #[serde(default)]
    pub categories: Vec<String>,
    #[serde(default)]
    pub versions: Vec<String>,
    #[serde(default)]
    pub downloads: u64,
}

#[derive(Debug, Clone)]
pub struct ModSearchResult {
    pub hits: Vec<ModSearchHit>,
    pub total_hits: u64,
}

#[derive(Debug, Deserialize)]
struct SearchResponse {
    #[serde(default)]
    hits: Vec<ModSearchHit>,
    #[serde(default)]
    total_hits: u64,
}

pub struct ModSearchRequest<'a> {
    pub query: &'a str,
    pub project_type: ProjectType,
    pub game_version: Option<&'a str>,
    /// Modrinth loader facet, e.g. `fabric`, `quilt`, `forge`, `neoforge`.
    pub loader: Option<&'a str>,
    pub offset: u32,
}

#[derive(Debug, Deserialize)]
struct ModVersion {
    id: String,
    #[serde(default)]
    version_number: String,
    #[serde(default)]
    files: Vec<ModFile>,
    #[serde(default)]
    dependencies: Vec<VersionDependency>,
}

#[derive(Debug, Deserialize)]
struct ModFile {
    url: String,
    filename: String,
    #[serde(default)]
    size: u64,
    #[serde(default)]
    primary: bool,
}

#[derive(Debug, Deserialize)]
struct VersionDependency {
    #[serde(default)]
    project_id: Option<String>,
    #[serde(default)]
    version_id: Option<String>,
    #[serde(default)]
    dependency_type: String,
}

/// A mod that was written into the mods directory.
#[derive(Debug, Clone)]
pub struct InstalledMod {
    pub project_id: String,
    pub filename: String,
    pub version_id: String,
    pub version_number: String,
    /// Project ids of the required dependencies.
    pub dependencies: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct InstallOutcome {
    /// The requested mod first, then the dependencies that were installed.
    pub installed: Vec<InstalledMod>,
    /// Required dependencies that had no compatible version.
    pub skipped: Vec<String>,
}

impl InstallOutcome {
    pub fn root(&self) -> Option<&InstalledMod> {
        self.installed.first()
    }
}

/// Search Modrinth for projects of `request.project_type` matching the filters.
pub async fn search(request: ModSearchRequest<'_>) -> Result<ModSearchResult, ModrinthError> {
    let mut facets: Vec<Vec<String>> =
        vec![vec![format!("project_type:{}", request.project_type.facet())]];
    if let Some(game_version) = request.game_version.filter(|value| !value.trim().is_empty()) {
        facets.push(vec![format!("versions:{game_version}")]);
    }
    if let Some(loader) = request.loader {
        facets.push(vec![format!("categories:{loader}")]);
    }

    let params: Vec<(&str, String)> = vec![
        ("query", request.query.to_owned()),
        ("facets", serde_json::to_string(&facets).unwrap_or_default()),
        ("offset", request.offset.to_string()),
        ("limit", PAGE_SIZE.to_string()),
    ];
    let response = client()?
        .get(SEARCH_URL)
        .query(&params)
        .send()
        .await?
        .error_for_status()?
        .json::<SearchResponse>()
        .await?;

    Ok(ModSearchResult {
        hits: response.hits,
        total_hits: response.total_hits,
    })
}

/// 分块下载单个文件，同时向 `progress` 汇报字节进度。
async fn download_bytes(
    client: &reqwest::Client,
    file: &ModFile,
    progress: &mut (dyn FnMut(ContentProgress) + Send),
) -> Result<Vec<u8>, ModrinthError> {
    let mut response = client
        .get(file.url.as_str())
        .send()
        .await?
        .error_for_status()?;
    let total = file.size;
    let mut data: Vec<u8> = Vec::with_capacity((total as usize).min(16 * 1024 * 1024));
    let mut downloaded: u64 = 0;
    let mut last_emit = std::time::Instant::now();
    while let Some(chunk) = response.chunk().await? {
        data.extend_from_slice(&chunk);
        downloaded += chunk.len() as u64;
        let finished = total > 0 && downloaded >= total;
        if finished || last_emit.elapsed() >= CONTENT_PROGRESS_INTERVAL {
            last_emit = std::time::Instant::now();
            progress(ContentProgress {
                filename: file.filename.clone(),
                downloaded,
                total,
            });
        }
    }
    Ok(data)
}

/// Install `project` together with its required dependencies into `mods_dir`.
pub async fn install_with_dependencies(
    project: &str,
    game_version: &str,
    loader: &str,
    mods_dir: &Path,
    progress: &mut (dyn FnMut(ContentProgress) + Send),
) -> Result<InstallOutcome, ModrinthError> {
    let client = client()?;
    std::fs::create_dir_all(mods_dir)?;
    let mut installed = Vec::new();
    let mut skipped = Vec::new();
    let mut visited = HashSet::new();
    // (project id, 是否作为前置安装, 作者指定/兼容的最新版本 id)
    let mut queue: VecDeque<(String, bool, Option<String>)> = VecDeque::new();
    queue.push_back((project.to_owned(), false, None));

    while let Some((project_id, auto, pinned)) = queue.pop_front() {
        if !visited.insert(project_id.clone()) {
            continue;
        }
        if installed.len() >= MAX_INSTALLED_MODS {
            break;
        }

        // 前置优先用作者指定的版本，拿不到再退回当前游戏版本下的最新兼容版本
        let version = match pinned {
            Some(version_id) => fetch_version(&client, &version_id).await.ok(),
            None => None,
        };
        let version = match version {
            Some(version) => Some(version),
            None => fetch_versions(&client, &project_id, game_version, Some(loader))
                .await?
                .into_iter()
                .next(),
        };
        let Some(version) = version else {
            if auto {
                skipped.push(project_id);
                continue;
            }
            return Err(ModrinthError::NoCompatibleVersion(project_id));
        };

        let mut files = version.files;
        let file = match files.iter().position(|file| file.primary) {
            Some(index) => files.remove(index),
            None if files.is_empty() => {
                if auto {
                    skipped.push(project_id);
                    continue;
                }
                return Err(ModrinthError::NoFile(project_id));
            }
            None => files.remove(0),
        };

        let bytes = download_bytes(&client, &file, &mut *progress).await?;
        let filename = sanitize_filename(&file.filename);
        std::fs::write(mods_dir.join(&filename), bytes)?;

        let required: Vec<(String, Option<String>)> = version
            .dependencies
            .iter()
            .filter(|dependency| dependency.dependency_type == "required")
            .filter_map(|dependency| {
                dependency
                    .project_id
                    .clone()
                    .map(|project| (project, dependency.version_id.clone()))
            })
            .collect();
        installed.push(InstalledMod {
            project_id: project_id.clone(),
            filename,
            version_id: version.id,
            version_number: version.version_number,
            dependencies: required
                .iter()
                .map(|(dependency, _)| dependency.clone())
                .collect(),
        });
        for (dependency, pinned) in required {
            queue.push_back((dependency, true, pinned));
        }
    }

    Ok(InstallOutcome { installed, skipped })
}

/// Install one resource pack, shader pack or data pack without a dependency chain.
pub async fn install_content(
    project: &str,
    game_version: &str,
    target_dir: &Path,
    progress: &mut (dyn FnMut(ContentProgress) + Send),
) -> Result<InstallOutcome, ModrinthError> {
    let client = client()?;
    std::fs::create_dir_all(target_dir)?;
    let versions = fetch_versions(&client, project, game_version, None).await?;
    let Some(version) = versions.into_iter().next() else {
        return Err(ModrinthError::NoCompatibleVersion(project.to_owned()));
    };
    let mut files = version.files;
    let file = match files.iter().position(|file| file.primary) {
        Some(index) => files.remove(index),
        None if files.is_empty() => return Err(ModrinthError::NoFile(project.to_owned())),
        None => files.remove(0),
    };
    let bytes = download_bytes(&client, &file, progress).await?;
    let filename = sanitize_filename(&file.filename);
    std::fs::write(target_dir.join(&filename), bytes)?;
    Ok(InstallOutcome {
        installed: vec![InstalledMod {
            project_id: project.to_owned(),
            filename,
            version_id: version.id,
            version_number: version.version_number,
            dependencies: Vec::new(),
        }],
        skipped: Vec::new(),
    })
}

/// Download the primary file of the newest compatible version of `project` to `dest`.
///
/// Used for modpacks: the archive is downloaded first and then installed by
/// [`crate::core::modpack`].
pub async fn download_primary_file(
    project: &str,
    game_version: &str,
    loader: Option<&str>,
    dest: &Path,
    progress: &mut (dyn FnMut(ContentProgress) + Send),
) -> Result<(), ModrinthError> {
    let client = client()?;
    let versions = fetch_versions(&client, project, game_version, loader).await?;
    let Some(version) = versions.into_iter().next() else {
        return Err(ModrinthError::NoCompatibleVersion(project.to_owned()));
    };
    let mut files = version.files;
    let file = match files.iter().position(|file| file.primary) {
        Some(index) => files.remove(index),
        None if files.is_empty() => return Err(ModrinthError::NoFile(project.to_owned())),
        None => files.remove(0),
    };
    let bytes = download_bytes(&client, &file, progress).await?;
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(dest, bytes)?;
    Ok(())
}

/// Latest compatible version of `project`, as `(version id, version number)`.
pub async fn latest_version(
    project: &str,
    game_version: &str,
    loader: Option<&str>,
) -> Result<Option<(String, String)>, ModrinthError> {
    let client = client()?;
    let versions = fetch_versions(&client, project, game_version, loader).await?;
    Ok(versions
        .into_iter()
        .next()
        .map(|version| (version.id, version.version_number)))
}

/// 远程图标的本地缓存路径；缓存目录不可用时返回 `None`。
///
/// 文件名用 URL 哈希，保留原扩展名让加载器按格式解码。
pub fn icon_cache_path(icon_url: &str) -> Option<PathBuf> {
    let mut hasher = DefaultHasher::new();
    icon_url.hash(&mut hasher);
    let stem = format!("{:016x}", hasher.finish());
    let file_name = match image_extension(icon_url) {
        Some(ext) => format!("{stem}.{ext}"),
        None => stem,
    };
    Some(config::cache_dir().ok()?.join("icons").join(file_name))
}

/// 把远程图标下载到缓存文件，已缓存时跳过。
pub async fn cache_icon(icon_url: &str) -> Result<(), ModrinthError> {
    let Some(path) = icon_cache_path(icon_url) else {
        return Ok(());
    };
    if path.is_file() {
        return Ok(());
    }
    let bytes = client()?
        .get(icon_url)
        .send()
        .await?
        .error_for_status()?
        .bytes()
        .await?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // 界面在写入过程中读到残缺文件会缓存解码失败，因此先写临时文件再原子改名。
    let stem = path.file_stem().and_then(|stem| stem.to_str()).unwrap_or("icon");
    let temp = path.with_file_name(format!("{stem}.{}.part", std::process::id()));
    std::fs::write(&temp, bytes)?;
    if let Err(error) = std::fs::rename(&temp, &path) {
        let _ = std::fs::remove_file(&temp);
        // 并发下载同一图标时目标可能已存在，此时缓存已就绪
        if !path.is_file() {
            return Err(error.into());
        }
    }
    Ok(())
}

fn image_extension(url: &str) -> Option<String> {
    let name = url.rsplit('/').next()?.split(['?', '#']).next()?;
    let ext = name.rsplit_once('.')?.1.to_ascii_lowercase();
    matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "webp" | "gif").then_some(ext)
}

async fn fetch_versions(
    client: &reqwest::Client,
    project: &str,
    game_version: &str,
    loader: Option<&str>,
) -> Result<Vec<ModVersion>, ModrinthError> {
    let mut params: Vec<(&str, String)> = vec![(
        "game_versions",
        serde_json::to_string(&[game_version]).unwrap_or_default(),
    )];
    if let Some(loader) = loader.filter(|loader| !loader.trim().is_empty()) {
        params.push((
            "loaders",
            serde_json::to_string(&[loader]).unwrap_or_default(),
        ));
    }
    let url = format!("https://api.modrinth.com/v2/project/{project}/version");
    let response = client.get(url).query(&params).send().await?;
    // 404 意味着项目名写错，直接给出项目标识而不是原始 URL
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Err(ModrinthError::ProjectNotFound(project.to_owned()));
    }
    Ok(response.error_for_status()?.json::<Vec<ModVersion>>().await?)
}

async fn fetch_version(
    client: &reqwest::Client,
    version_id: &str,
) -> Result<ModVersion, ModrinthError> {
    Ok(client
        .get(format!("{VERSION_URL}/{version_id}"))
        .send()
        .await?
        .error_for_status()?
        .json::<ModVersion>()
        .await?)
}

fn sanitize_filename(filename: &str) -> String {
    // 文件名来自外部 API，只取最后一段，避免路径穿越
    Path::new(filename)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("mod.jar")
        .to_owned()
}

fn client() -> Result<reqwest::Client, ModrinthError> {
    Ok(reqwest::Client::builder().user_agent(USER_AGENT).build()?)
}
