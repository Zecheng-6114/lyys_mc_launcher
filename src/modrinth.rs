use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::download::{self, Origins};

const API: &str = "https://api.modrinth.com/v2";

pub struct Hit {
    pub slug: String,
    pub title: String,
    pub downloads: u64,
    pub description: String,
    pub project_type: String,
}

pub fn search(
    query: &str,
    project_type: &str,
    loaders: &[String],
    versions: &[String],
    limit: u32,
) -> Result<Vec<Hit>, String> {
    let mut facets: Vec<String> = vec![format!("[\"project_type:{project_type}\"]")];

    if !loaders.is_empty() {
        facets.push(facet("categories", loaders));
    }
    if !versions.is_empty() {
        facets.push(facet("versions", versions));
    }

    let url = format!(
        "{API}/search?query={}&limit={}&facets={}",
        encode(query),
        limit,
        encode(&format!("[{}]", facets.join(",")))
    );

    let body = download::get_json(&url)?;
    let hits = body["hits"]
        .as_array()
        .ok_or_else(|| String::from("modrinth: response has no hits"))?;

    Ok(hits
        .iter()
        .map(|hit| Hit {
            slug: hit["slug"].as_str().unwrap_or_default().to_string(),
            title: hit["title"].as_str().unwrap_or_default().to_string(),
            downloads: hit["downloads"].as_u64().unwrap_or(0),
            description: hit["description"].as_str().unwrap_or_default().to_string(),
            project_type: hit["project_type"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
        })
        .collect())
}

pub struct OptionalDep {
    pub name: String,
    pub installed: bool,
}

pub struct InstallReport {
    pub files: Vec<String>,
    pub optional: Vec<OptionalDep>,
}

pub fn install(
    mc_dir: &str,
    instance: &str,
    slug: &str,
    project_type: &str,
    loaders: &[String],
    versions: &[String],
    with_optional: bool,
) -> Result<InstallReport, String> {
    let target = match project_type {
        "mod" => "mods",
        "resourcepack" => "resourcepacks",
        "shader" => "shaderpacks",
        "datapack" => "datapacks",
        other => return Err(format!("unsupported project_type: {other}")),
    };

    let version_json = Path::new(mc_dir)
        .join("versions")
        .join(instance)
        .join(format!("{instance}.json"));
    if !version_json.exists() {
        return Err(format!("instance {instance} not found"));
    }

    let target_dir = Path::new(mc_dir).join("versions").join(instance).join(target);
    std::fs::create_dir_all(&target_dir)
        .map_err(|e| format!("failed to create dir {}: {e}", target_dir.display()))?;

    let mut queue: Vec<Value> = vec![latest_version(slug, loaders, versions)?];
    let mut visited: Vec<String> = Vec::new();
    let mut files: Vec<String> = Vec::new();
    let mut optional: Vec<OptionalDep> = Vec::new();

    while let Some(version) = queue.pop() {
        let id = version["id"].as_str().unwrap_or_default().to_string();
        if !id.is_empty() {
            if visited.contains(&id) {
                continue;
            }
            visited.push(id);
        }

        files.push(download_version(&version, &target_dir)?);

        for dep in version["dependencies"].as_array().cloned().unwrap_or_default() {
            match dep["dependency_type"].as_str() {
                Some("required") => {
                    let next = if let Some(version_id) = dep["version_id"].as_str() {
                        version_by_id(version_id)?
                    } else if let Some(project_id) = dep["project_id"].as_str() {
                        latest_version(project_id, loaders, versions)?
                    } else {
                        continue;
                    };
                    queue.push(next);
                }
                Some("optional") => {
                    let name = dep["project_id"]
                        .as_str()
                        .and_then(project_title)
                        .unwrap_or_else(|| String::from("unknown"));

                    if with_optional {
                        let next = if let Some(version_id) = dep["version_id"].as_str() {
                            version_by_id(version_id)?
                        } else if let Some(project_id) = dep["project_id"].as_str() {
                            latest_version(project_id, loaders, versions)?
                        } else {
                            continue;
                        };
                        queue.push(next);
                        optional.push(OptionalDep {
                            name,
                            installed: true,
                        });
                    } else {
                        optional.push(OptionalDep {
                            name,
                            installed: false,
                        });
                    }
                }
                _ => {}
            }
        }
    }

    Ok(InstallReport { files, optional })
}

fn project_title(project_id: &str) -> Option<String> {
    let project = download::get_json(&format!("{API}/project/{project_id}")).ok()?;
    project["title"].as_str().map(|title| title.to_string())
}

fn latest_version(slug: &str, loaders: &[String], versions: &[String]) -> Result<Value, String> {
    let mut params: Vec<String> = Vec::new();
    if !loaders.is_empty() {
        params.push(format!("loaders={}", encode(&array_json(loaders))));
    }
    if !versions.is_empty() {
        params.push(format!("game_versions={}", encode(&array_json(versions))));
    }

    let suffix = if params.is_empty() {
        String::new()
    } else {
        format!("?{}", params.join("&"))
    };

    let body = download::get_json(&format!("{API}/project/{slug}/version{suffix}"))?;
    let list = body
        .as_array()
        .ok_or_else(|| String::from("modrinth: malformed version list"))?;

    list.first()
        .cloned()
        .ok_or_else(|| format!("no matching version for {slug}"))
}

fn version_by_id(id: &str) -> Result<Value, String> {
    download::get_json(&format!("{API}/version/{id}"))
}

fn download_version(version: &Value, target_dir: &Path) -> Result<String, String> {
    let files = version["files"]
        .as_array()
        .ok_or_else(|| String::from("modrinth: version has no files"))?;
    let file = files
        .iter()
        .find(|file| file["primary"].as_bool() == Some(true))
        .or_else(|| files.first())
        .ok_or_else(|| String::from("modrinth: version files are empty"))?;

    let url = file["url"]
        .as_str()
        .ok_or_else(|| String::from("missing download url"))?;
    let filename = file["filename"]
        .as_str()
        .ok_or_else(|| String::from("missing filename"))?;

    let dest: PathBuf = target_dir.join(filename);
    let plain = Origins {
        mojang: None,
        maven: None,
    };
    download::fetch(url, &dest, None, &plain)?;

    Ok(filename.to_string())
}

fn facet(key: &str, values: &[String]) -> String {
    let inner: Vec<String> = values
        .iter()
        .map(|value| format!("\"{key}:{value}\""))
        .collect();
    format!("[{}]", inner.join(","))
}

fn array_json(values: &[String]) -> String {
    let inner: Vec<String> = values.iter().map(|value| format!("\"{value}\"")).collect();
    format!("[{}]", inner.join(","))
}

fn encode(input: &str) -> String {
    let mut out = String::new();
    for byte in input.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}
