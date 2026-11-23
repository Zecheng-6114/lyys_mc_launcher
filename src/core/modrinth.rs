use std::collections::{HashSet, VecDeque};
use std::path::Path;

use serde::Deserialize;
use thiserror::Error;

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
    #[error("no compatible version found for {0}")]
    NoCompatibleVersion(String),
    #[error("{0} has no downloadable file")]
    NoFile(String),
    #[error("failed to save the mod: {0}")]
    Io(#[from] std::io::Error),
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

/// Search Modrinth for mods matching the query and optional version/loader filters.
pub async fn search(request: ModSearchRequest<'_>) -> Result<ModSearchResult, ModrinthError> {
    let mut facets: Vec<Vec<String>> = vec![vec!["project_type:mod".to_owned()]];
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

/// Install `project` together with its required dependencies into `mods_dir`.
pub async fn install_with_dependencies(
    project: &str,
    game_version: &str,
    loader: &str,
    mods_dir: &Path,
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
            None => fetch_versions(&client, &project_id, game_version, loader)
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

        let bytes = client
            .get(file.url.as_str())
            .send()
            .await?
            .error_for_status()?
            .bytes()
            .await?;
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

/// Latest compatible version of `project`, as `(version id, version number)`.
pub async fn latest_version(
    project: &str,
    game_version: &str,
    loader: &str,
) -> Result<Option<(String, String)>, ModrinthError> {
    let client = client()?;
    let versions = fetch_versions(&client, project, game_version, loader).await?;
    Ok(versions
        .into_iter()
        .next()
        .map(|version| (version.id, version.version_number)))
}

async fn fetch_versions(
    client: &reqwest::Client,
    project: &str,
    game_version: &str,
    loader: &str,
) -> Result<Vec<ModVersion>, ModrinthError> {
    let params: Vec<(&str, String)> = vec![
        (
            "game_versions",
            serde_json::to_string(&[game_version]).unwrap_or_default(),
        ),
        ("loaders", serde_json::to_string(&[loader]).unwrap_or_default()),
    ];
    let url = format!("https://api.modrinth.com/v2/project/{project}/version");
    Ok(client
        .get(url)
        .query(&params)
        .send()
        .await?
        .error_for_status()?
        .json::<Vec<ModVersion>>()
        .await?)
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
