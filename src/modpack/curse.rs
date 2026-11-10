use std::collections::HashMap;

use serde_json::Value;

use crate::download;
use crate::modpack::archive;
use crate::modpack::{LoaderKind, LoaderSpec, PackFile, PackManifest, Provider};

const CURSE_API: &str = "https://api.curseforge.com";
const CURSE_CDN: &str = "https://edge.forgecdn.net/files";

pub struct CurseClient {
    api_key: Option<String>,
    classes: HashMap<i64, i64>,
}

pub struct CurseFile {
    pub file_name: String,
    pub url: String,
    pub sha1: Option<String>,
    pub class_id: Option<i64>,
}

impl CurseFile {
    pub fn category(&self) -> &'static str {
        match self.class_id {
            Some(12) => "resourcepacks",
            Some(6945) => "datapacks",
            Some(6552) => "shaderpacks",
            _ => "mods",
        }
    }
}

impl CurseClient {
    pub fn new() -> Self {
        CurseClient {
            api_key: std::env::var("CURSEFORGE_API_KEY")
                .ok()
                .filter(|key| !key.trim().is_empty()),
            classes: HashMap::new(),
        }
    }

    pub fn resolve_file(
        &mut self,
        project_id: i64,
        file_id: i64,
        file_name: Option<&str>,
        url: Option<&str>,
    ) -> Result<CurseFile, String> {
        self.resolve(project_id, file_id, file_name, url)
    }

    fn resolve(
        &mut self,
        project_id: i64,
        file_id: i64,
        file_name: Option<&str>,
        url: Option<&str>,
    ) -> Result<CurseFile, String> {
        if let (Some(file_name), Some(url)) = (file_name, url)
            && !file_name.is_empty()
            && !url.is_empty()
        {
            return Ok(CurseFile {
                file_name: file_name.to_string(),
                url: url.to_string(),
                sha1: None,
                class_id: self.class_id(project_id).ok(),
            });
        }

        let key = self.api_key.clone().ok_or_else(|| {
            format!(
                "CurseForge file {project_id}/{file_id} only has projectID/fileID; set CURSEFORGE_API_KEY or use a pack whose manifest already contains fileName/url"
            )
        })?;

        let data = download::get_json_with_headers(
            &format!("{CURSE_API}/v1/mods/{project_id}/files/{file_id}"),
            &[("X-API-KEY", &key)],
        )?
        .get("data")
        .cloned()
        .ok_or_else(|| format!("CurseForge file {project_id}/{file_id} has no data"))?;

        let file_name = data["fileName"]
            .as_str()
            .ok_or_else(|| format!("CurseForge file {project_id}/{file_id} has no fileName"))?
            .to_string();
        let url = data["downloadUrl"]
            .as_str()
            .filter(|url| !url.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| curse_cdn_url(file_id, &file_name));
        let sha1 = data["hashes"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|hash| hash["algo"].as_i64() == Some(1))
            .and_then(|hash| hash["value"].as_str())
            .map(str::to_string);

        Ok(CurseFile {
            file_name,
            url,
            sha1,
            class_id: self.class_id(project_id).ok(),
        })
    }

    fn class_id(&mut self, project_id: i64) -> Result<i64, String> {
        if let Some(class_id) = self.classes.get(&project_id) {
            return Ok(*class_id);
        }

        let key = self
            .api_key
            .clone()
            .ok_or_else(|| String::from("CURSEFORGE_API_KEY is required for file placement"))?;
        let data = download::get_json_with_headers(
            &format!("{CURSE_API}/v1/mods/{project_id}"),
            &[("X-API-KEY", &key)],
        )?
        .get("data")
        .cloned()
        .ok_or_else(|| format!("CurseForge project {project_id} has no data"))?;
        let class_id = data["classId"]
            .as_i64()
            .ok_or_else(|| format!("CurseForge project {project_id} has no classId"))?;
        self.classes.insert(project_id, class_id);
        Ok(class_id)
    }
}

pub fn parse(
    manifest: &Value,
    client: &mut CurseClient,
    with_optional: bool,
) -> Result<PackManifest, String> {
    if manifest["manifestType"].as_str() != Some("minecraftModpack") {
        return Err(String::from(
            "CurseForge manifest.json has unsupported manifestType",
        ));
    }

    let game_version = manifest["minecraft"]["version"]
        .as_str()
        .ok_or_else(|| String::from("CurseForge manifest is missing minecraft.version"))?
        .to_string();

    let loader = parse_loader(&manifest["minecraft"]["modLoaders"])?;
    let mut files = Vec::new();

    for entry in manifest["files"].as_array().into_iter().flatten() {
        let project_id = entry["projectID"]
            .as_i64()
            .ok_or_else(|| String::from("CurseForge file has no projectID"))?;
        let file_id = entry["fileID"]
            .as_i64()
            .ok_or_else(|| String::from("CurseForge file has no fileID"))?;
        let required = entry["required"].as_bool().unwrap_or(true);
        if !required
            && !with_optional
            && entry["fileName"].as_str().is_none()
            && entry["url"].as_str().is_none()
        {
            continue;
        }

        let resolved = client.resolve(
            project_id,
            file_id,
            entry["fileName"].as_str(),
            entry["url"].as_str(),
        )?;

        let path =
            archive::safe_relative(&format!("{}/{}", resolved.category(), resolved.file_name))?
                .to_string_lossy()
                .replace('\\', "/");

        let sha1 = entry["hashes"]
            .as_array()
            .and_then(|hashes| {
                hashes
                    .iter()
                    .find(|hash| hash["algo"].as_i64() == Some(1))
                    .and_then(|hash| hash["value"].as_str())
            })
            .map(str::to_string)
            .or(resolved.sha1);

        files.push(PackFile {
            path,
            size: entry["fileLength"].as_u64(),
            sha1,
            sha512: None,
            urls: vec![resolved.url],
            optional: !required,
            client_unsupported: false,
        });
    }

    Ok(PackManifest {
        provider: Provider::CurseForge,
        name: manifest["name"]
            .as_str()
            .unwrap_or("CurseForge pack")
            .to_string(),
        version: manifest["version"]
            .as_str()
            .unwrap_or("unknown")
            .to_string(),
        author: manifest["author"].as_str().unwrap_or_default().to_string(),
        description: String::new(),
        game_version,
        loader,
        files,
        overrides: vec![
            manifest["overrides"]
                .as_str()
                .filter(|value| !value.is_empty())
                .unwrap_or("overrides")
                .to_string(),
        ],
        libraries: Vec::new(),
        manifest_name: String::from("manifest.json"),
    })
}

fn parse_loader(mod_loaders: &Value) -> Result<Option<LoaderSpec>, String> {
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
        .ok_or_else(|| String::from("CurseForge mod loader has no id"))?;
    let (kind, version) = if let Some(version) = id.strip_prefix("forge-") {
        (LoaderKind::Forge, version)
    } else if let Some(version) = id.strip_prefix("fabric-") {
        (LoaderKind::Fabric, version)
    } else if let Some(version) = id.strip_prefix("neoforge-") {
        (LoaderKind::NeoForge, version)
    } else if id.starts_with("quilt-") {
        return Err(String::from(
            "Quilt Loader is not supported yet; only Fabric, Forge and NeoForge are supported",
        ));
    } else {
        return Err(format!("unsupported CurseForge mod loader: {id}"));
    };

    Ok(Some(LoaderSpec {
        kind,
        version: Some(version.to_string()),
    }))
}

fn curse_cdn_url(file_id: i64, file_name: &str) -> String {
    format!(
        "{CURSE_CDN}/{}/{}/{}",
        file_id / 1000,
        file_id % 1000,
        download::encode_component(file_name)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_enriched_manifest_without_api_key() {
        let manifest = serde_json::json!({
            "manifestType": "minecraftModpack",
            "name": "Example",
            "version": "1.0",
            "author": "author",
            "minecraft": {
                "version": "1.20.1",
                "modLoaders": [
                    {"id": "forge-47.2.0", "primary": true}
                ]
            },
            "files": [
                {
                    "projectID": 1,
                    "fileID": 2,
                    "required": true,
                    "fileName": "example.jar",
                    "url": "https://example.invalid/example.jar"
                },
                {
                    "projectID": 3,
                    "fileID": 4,
                    "required": false,
                    "fileName": "optional.jar",
                    "url": "https://example.invalid/optional.jar"
                }
            ]
        });

        let mut client = CurseClient::new();
        let pack = parse(&manifest, &mut client, true).unwrap();
        assert_eq!(pack.loader.as_ref().unwrap().kind, LoaderKind::Forge);
        assert_eq!(pack.files[0].path, "mods/example.jar");
        assert!(pack.files[1].optional);
    }

    #[test]
    fn builds_cdn_url() {
        assert_eq!(
            curse_cdn_url(12345, "a.jar"),
            "https://edge.forgecdn.net/files/12/345/a.jar"
        );
    }
}
