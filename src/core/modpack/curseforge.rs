//! CurseForge 整合包（`manifest.json`）。
//!
//! 清单里通常只有 projectID/fileID；解析下载地址需要 `CURSEFORGE_API_KEY`，
//! 若清单已自带 `fileName` 与 `url` 则不需要联网。

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::Value;
use tokio::task::JoinSet;

use super::{
    METADATA_CONCURRENCY, ModpackError, PackFile, PackManifest, Provider, encode_path, http_client,
    loader_spec, safe_relative,
};

const CURSE_API: &str = "https://api.curseforge.com";
const CURSE_CDN: &str = "https://edge.forgecdn.net/files";
/// 元数据请求的最大尝试次数。
const META_ATTEMPTS: usize = 3;

#[derive(Clone)]
pub struct CurseFile {
    pub file_name: String,
    pub url: String,
    pub sha1: Option<String>,
    pub class_id: Option<i64>,
}

impl CurseFile {
    pub fn category(&self) -> &'static str {
        category_of(self.class_id, &self.file_name)
    }
}

fn category_of(class_id: Option<i64>, file_name: &str) -> &'static str {
    match class_id {
        Some(12) => "resourcepacks",
        Some(6945) => "datapacks",
        Some(6552) => "shaderpacks",
        _ => {
            if file_name.to_ascii_lowercase().ends_with(".zip") {
                "resourcepacks"
            } else {
                "mods"
            }
        }
    }
}

/// 一个待解析的清单条目；持有 String 便于并发任务直接 move 进去。
#[derive(Clone)]
pub(crate) struct FileRequest {
    pub project_id: i64,
    pub file_id: i64,
    pub file_name: Option<String>,
    pub url: Option<String>,
    pub class_id: Option<i64>,
}

/// CurseForge API 客户端，按项目缓存 classId，可在多个任务间共享。
pub struct Client {
    api_key: Option<String>,
    classes: Mutex<HashMap<i64, i64>>,
    http: reqwest::Client,
}

impl Client {
    pub(crate) fn new() -> Self {
        Self {
            api_key: std::env::var("CURSEFORGE_API_KEY")
                .ok()
                .filter(|key| !key.trim().is_empty()),
            classes: Mutex::new(HashMap::new()),
            http: http_client().clone(),
        }
    }

    pub(crate) async fn resolve_file(&self, request: &FileRequest) -> Result<CurseFile, ModpackError> {
        let FileRequest {
            project_id,
            file_id,
            file_name,
            url,
            class_id,
        } = request;
        if let (Some(file_name), Some(url)) = (file_name, url)
            && !file_name.is_empty()
            && !url.is_empty()
        {
            let class_id = match class_id {
                Some(class_id) => Some(*class_id),
                None => self.class_id(*project_id).await.ok(),
            };
            return Ok(CurseFile {
                file_name: file_name.clone(),
                url: url.clone(),
                sha1: None,
                class_id,
            });
        }

        let data = self
            .get_json(&format!("{CURSE_API}/v1/mods/{project_id}/files/{file_id}"))
            .await?
            .get("data")
            .cloned()
            .ok_or_else(|| {
                ModpackError::Parse(format!(
                    "CurseForge file {project_id}/{file_id} has no data"
                ))
            })?;
        let file_name = data["fileName"].as_str().ok_or_else(|| {
            ModpackError::Parse(format!(
                "CurseForge file {project_id}/{file_id} has no fileName"
            ))
        })?;
        let url = data["downloadUrl"]
            .as_str()
            .filter(|url| !url.is_empty())
            .map(str::to_owned)
            .unwrap_or_else(|| curse_cdn_url(*file_id, file_name));
        let sha1 = data["hashes"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|hash| hash["algo"].as_i64() == Some(1))
            .and_then(|hash| hash["value"].as_str())
            .map(str::to_owned);
        let class_id = match class_id {
            Some(class_id) => Some(*class_id),
            None => self.class_id(*project_id).await.ok(),
        };
        Ok(CurseFile {
            file_name: file_name.to_owned(),
            url,
            sha1,
            class_id,
        })
    }

    async fn class_id(&self, project_id: i64) -> Result<i64, ModpackError> {
        if let Some(class_id) = self.cached_class(project_id) {
            return Ok(class_id);
        }
        let data = self
            .get_json(&format!("{CURSE_API}/v1/mods/{project_id}"))
            .await?
            .get("data")
            .cloned()
            .ok_or_else(|| {
                ModpackError::Parse(format!("CurseForge project {project_id} has no data"))
            })?;
        let class_id = data["classId"].as_i64().ok_or_else(|| {
            ModpackError::Parse(format!("CurseForge project {project_id} has no classId"))
        })?;
        if let Ok(mut classes) = self.classes.lock() {
            classes.insert(project_id, class_id);
        }
        Ok(class_id)
    }

    fn cached_class(&self, project_id: i64) -> Option<i64> {
        self.classes
            .lock()
            .ok()
            .and_then(|classes| classes.get(&project_id).copied())
    }

    async fn get_json(&self, url: &str) -> Result<Value, ModpackError> {
        let key = self.api_key.as_deref().ok_or_else(|| {
            ModpackError::Parse(
                "CURSEFORGE_API_KEY is required to resolve CurseForge files".to_owned(),
            )
        })?;
        let mut last_error = None;
        for attempt in 1..=META_ATTEMPTS {
            let response = match self
                .http
                .get(url)
                .header("X-API-KEY", key)
                .header("Accept", "application/json")
                .send()
                .await
            {
                Ok(response) => response,
                Err(error) => {
                    // 连接被重置之类的临时问题，退避后重试
                    last_error = Some(error.to_string());
                    if attempt < META_ATTEMPTS {
                        tokio::time::sleep(backoff(attempt)).await;
                    }
                    continue;
                }
            };
            let status = response.status();
            if status.is_success() {
                return Ok(response.json().await?);
            }
            last_error = Some(format!("CurseForge API returned {status} for {url}"));
            let transient =
                status.is_server_error() || status == reqwest::StatusCode::TOO_MANY_REQUESTS;
            if transient && attempt < META_ATTEMPTS {
                tokio::time::sleep(backoff(attempt)).await;
                continue;
            }
            break;
        }
        Err(ModpackError::Parse(last_error.unwrap_or_else(|| {
            format!("CurseForge request failed: {url}")
        })))
    }
}

fn backoff(attempt: usize) -> Duration {
    Duration::from_millis(400 * attempt as u64)
}

/// 有界并发解析清单条目：同一 (projectID, fileID) 只请求一次，失败不影响其他条目。
pub(crate) async fn resolve_many(
    client: Arc<Client>,
    requests: Vec<FileRequest>,
) -> HashMap<(i64, i64), Result<CurseFile, String>> {
    let mut unique = Vec::new();
    let mut seen = HashSet::new();
    for request in requests {
        if seen.insert((request.project_id, request.file_id)) {
            unique.push(request);
        }
    }

    // 先按项目维度补齐 classId，避免同一项目的多个文件重复查询
    prefetch_classes(&client, &unique).await;

    let mut files = HashMap::with_capacity(unique.len());
    let mut set = JoinSet::new();
    let mut next = 0;
    while next < unique.len() || !set.is_empty() {
        while next < unique.len() && set.len() < METADATA_CONCURRENCY {
            let request = unique[next].clone();
            next += 1;
            let client = Arc::clone(&client);
            set.spawn(async move {
                let key = (request.project_id, request.file_id);
                (key, client.resolve_file(&request).await.map_err(|error| error.to_string()))
            });
        }
        if let Some(joined) = set.join_next().await {
            match joined {
                Ok((key, result)) => {
                    files.insert(key, result);
                }
                // 任务异常时该条目缺失，由调用方按缺失的键报错
                Err(_) => {}
            }
        }
    }
    files
}

/// 并发补齐缺失的 classId，结果写进客户端缓存。
async fn prefetch_classes(client: &Arc<Client>, requests: &[FileRequest]) {
    let mut project_ids = HashSet::new();
    for request in requests {
        if request.class_id.is_none() && client.cached_class(request.project_id).is_none() {
            project_ids.insert(request.project_id);
        }
    }
    let mut ids: Vec<i64> = project_ids.into_iter().collect();
    ids.sort_unstable();

    let mut set = JoinSet::new();
    let mut next = 0;
    while next < ids.len() || !set.is_empty() {
        while next < ids.len() && set.len() < METADATA_CONCURRENCY {
            let project_id = ids[next];
            next += 1;
            let client = Arc::clone(client);
            set.spawn(async move {
                // 没有 API Key 时查询会失败，这里静默跳过，交给文件解析阶段决定
                let _ = client.class_id(project_id).await;
            });
        }
        let _ = set.join_next().await;
    }
}

pub async fn parse(manifest: &Value) -> Result<PackManifest, ModpackError> {
    if manifest["manifestType"].as_str() != Some("minecraftModpack") {
        return Err(ModpackError::Parse(
            "manifest.json is not a CurseForge modpack".to_owned(),
        ));
    }
    let game_version = manifest["minecraft"]["version"]
        .as_str()
        .filter(|version| !version.trim().is_empty())
        .ok_or_else(|| ModpackError::Parse("manifest.json has no Minecraft version".to_owned()))?
        .to_owned();
    let loader = parse_loader(&manifest["minecraft"]["modLoaders"])?;

    struct Entry {
        project_id: i64,
        file_id: i64,
        size: Option<u64>,
        sha1: Option<String>,
        optional: bool,
    }

    let mut requests = Vec::new();
    let mut entries = Vec::new();
    for entry in manifest["files"].as_array().into_iter().flatten() {
        let Some(project_id) = entry["projectID"].as_i64() else {
            continue;
        };
        let Some(file_id) = entry["fileID"].as_i64() else {
            continue;
        };
        requests.push(FileRequest {
            project_id,
            file_id,
            file_name: entry["fileName"].as_str().map(str::to_owned),
            url: entry["url"].as_str().map(str::to_owned),
            class_id: entry["classId"].as_i64(),
        });
        entries.push(Entry {
            project_id,
            file_id,
            size: entry["fileLength"].as_u64(),
            sha1: entry["hashes"]
                .as_array()
                .and_then(|hashes| {
                    hashes
                        .iter()
                        .find(|hash| hash["algo"].as_i64() == Some(1))
                        .and_then(|hash| hash["value"].as_str())
                })
                .map(str::to_owned),
            optional: entry["required"].as_bool() == Some(false),
        });
    }

    let resolved = resolve_many(Arc::new(Client::new()), requests).await;
    let mut files = Vec::with_capacity(entries.len());
    for entry in entries {
        let resolved = resolved
            .get(&(entry.project_id, entry.file_id))
            .ok_or_else(|| {
                ModpackError::Download(format!(
                    "CurseForge file {}/{} could not be resolved",
                    entry.project_id, entry.file_id
                ))
            })?
            .as_ref()
            .map_err(|error| ModpackError::Download(error.to_owned()))?;
        let path = safe_relative(&format!("{}/{}", resolved.category(), resolved.file_name))?
            .to_string_lossy()
            .replace('\\', "/");
        files.push(PackFile {
            path,
            size: entry.size,
            sha1: entry.sha1.or_else(|| resolved.sha1.clone()),
            sha512: None,
            urls: vec![resolved.url.clone()],
            optional: entry.optional,
            client_unsupported: false,
        });
    }

    Ok(PackManifest {
        provider: Provider::CurseForge,
        name: manifest["name"]
            .as_str()
            .unwrap_or("CurseForge pack")
            .to_owned(),
        version: manifest["version"].as_str().unwrap_or("unknown").to_owned(),
        author: manifest["author"].as_str().unwrap_or_default().to_owned(),
        description: String::new(),
        game_version,
        loader,
        files,
        overrides: vec![
            manifest["overrides"]
                .as_str()
                .filter(|value| !value.is_empty())
                .unwrap_or("overrides")
                .to_owned(),
        ],
        manifest_name: "manifest.json".to_owned(),
    })
}

fn parse_loader(
    mod_loaders: &Value,
) -> Result<Option<mc_launcher_core::loader::common::LoaderSpec>, ModpackError> {
    let loaders = mod_loaders.as_array().cloned().unwrap_or_default();
    let Some(loader) = loaders
        .iter()
        .find(|loader| loader["primary"].as_bool() == Some(true))
        .or_else(|| loaders.first())
    else {
        return Ok(None);
    };
    let id = loader["id"]
        .as_str()
        .ok_or_else(|| ModpackError::Parse("CurseForge mod loader has no id".to_owned()))?;
    for (prefix, kind) in [
        ("forge-", "forge"),
        ("fabric-", "fabric"),
        ("neoforge-", "neoforge"),
        ("quilt-", "quilt"),
    ] {
        if let Some(version) = id.strip_prefix(prefix) {
            return Ok(loader_spec(kind, version));
        }
    }
    Err(ModpackError::Parse(format!(
        "unsupported CurseForge mod loader: {id}"
    )))
}

fn curse_cdn_url(file_id: i64, file_name: &str) -> String {
    format!(
        "{CURSE_CDN}/{}/{}/{}",
        file_id / 1000,
        file_id % 1000,
        encode_path(file_name)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_categories() {
        assert_eq!(category_of(Some(12), "x.jar"), "resourcepacks");
        assert_eq!(category_of(None, "pack.zip"), "resourcepacks");
        assert_eq!(category_of(None, "mod.jar"), "mods");
        assert_eq!(category_of(Some(6552), "shader.zip"), "shaderpacks");
    }

    #[test]
    fn builds_cdn_url() {
        assert_eq!(
            curse_cdn_url(4_567_890, "a b.jar"),
            "https://edge.forgecdn.net/files/4567/890/a%20b.jar"
        );
    }

    #[test]
    fn parses_loader_ids() {
        let loaders = serde_json::json!([{"id": "fabric-0.15.0", "primary": true}]);
        let loader = parse_loader(&loaders).unwrap().unwrap();
        assert!(matches!(
            loader,
            mc_launcher_core::loader::common::LoaderSpec::Fabric { .. }
        ));
    }
}
