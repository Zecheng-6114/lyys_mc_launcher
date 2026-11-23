//! 服务端整合包（`server-manifest.json`）。

use serde_json::Value;

use super::{ModpackError, PackFile, PackManifest, Provider, encode_path, loader_spec, safe_relative};

pub fn parse(manifest: &Value) -> Result<PackManifest, ModpackError> {
    let mut game_version = None;
    let mut loader = None;
    if let Some(addons) = manifest["addons"].as_array() {
        for addon in addons {
            let Some(id) = addon["id"].as_str() else {
                continue;
            };
            let version = addon["version"].as_str().map(str::trim).unwrap_or("");
            if id.eq_ignore_ascii_case("minecraft") {
                if !version.is_empty() {
                    game_version = Some(version.to_owned());
                }
                continue;
            }
            if loader.is_none() && !version.is_empty() {
                loader = loader_spec(&id.to_ascii_lowercase(), version);
            }
        }
    }
    let game_version = game_version
        .or_else(|| {
            manifest["minecraft"]["version"]
                .as_str()
                .map(str::to_owned)
                .filter(|version| !version.trim().is_empty())
        })
        .ok_or_else(|| {
            ModpackError::Parse("server pack has no Minecraft version".to_owned())
        })?;

    let file_api = manifest["fileApi"]
        .as_str()
        .map(|url| url.trim_end_matches('/').to_owned())
        .ok_or_else(|| ModpackError::Parse("server pack has no fileApi URL".to_owned()))?;

    let mut files = Vec::new();
    for entry in manifest["files"].as_array().into_iter().flatten() {
        let Some(path) = entry["path"].as_str() else {
            continue;
        };
        let relative = safe_relative(path)?;
        let path = relative.to_string_lossy().replace('\\', "/");
        files.push(PackFile {
            path: path.clone(),
            size: entry["size"].as_u64().or_else(|| entry["fileSize"].as_u64()),
            sha1: entry["hash"].as_str().map(str::to_owned),
            sha512: None,
            urls: vec![format!("{file_api}/overrides/{}", encode_path(&path))],
            optional: false,
            client_unsupported: false,
        });
    }

    Ok(PackManifest {
        provider: Provider::Server,
        name: manifest["name"].as_str().unwrap_or("Server pack").to_owned(),
        version: manifest["version"].as_str().unwrap_or("unknown").to_owned(),
        author: manifest["author"].as_str().unwrap_or_default().to_owned(),
        description: manifest["description"].as_str().unwrap_or_default().to_owned(),
        game_version,
        loader,
        files,
        overrides: vec!["overrides".to_owned()],
        manifest_name: "server-manifest.json".to_owned(),
    })
}
