//! MCBBS 整合包（`mcbbs.packmeta`，部分包也用 `manifest.json`）。

use serde_json::Value;

use super::curseforge::Client;
use super::{
    ModpackError, PackFile, PackManifest, Provider, encode_path, loader_spec, normalize_name,
    safe_relative,
};

pub async fn parse(manifest: &Value) -> Result<PackManifest, ModpackError> {
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
    if game_version.is_none() {
        game_version = manifest["minecraft"]["version"]
            .as_str()
            .map(str::to_owned)
            .filter(|version| !version.trim().is_empty());
    }
    let game_version = game_version.ok_or_else(|| {
        ModpackError::Parse("MCBBS pack has no Minecraft version".to_owned())
    })?;

    let install_mods = manifest["settings"]["install_mods"].as_bool().unwrap_or(true);
    let install_resourcepacks = manifest["settings"]["install_resourcepack"]
        .as_bool()
        .unwrap_or(true);
    let file_api = manifest["fileApi"]
        .as_str()
        .map(|url| url.trim_end_matches('/').to_owned());

    let mut client = Client::new();
    let mut files = Vec::new();
    for entry in manifest["files"].as_array().into_iter().flatten() {
        let kind = entry["type"].as_str().unwrap_or("addon");
        let optional = entry["required"].as_bool() == Some(false) || entry["force"].as_bool() == Some(false);
        if kind == "curse" {
            let Some(project_id) = entry["projectID"].as_i64() else {
                continue;
            };
            let Some(file_id) = entry["fileID"].as_i64() else {
                continue;
            };
            let resolved = client
                .resolve_file(
                    project_id,
                    file_id,
                    entry["fileName"].as_str(),
                    entry["url"].as_str(),
                    entry["classId"].as_i64(),
                )
                .await?;
            if !accept(&format!("{}/x", resolved.category()), install_mods, install_resourcepacks) {
                continue;
            }
            let path = safe_relative(&format!("{}/{}", resolved.category(), resolved.file_name))?
                .to_string_lossy()
                .replace('\\', "/");
            files.push(PackFile {
                path,
                size: entry["fileLength"].as_u64(),
                sha1: resolved.sha1,
                sha512: None,
                urls: vec![resolved.url],
                optional,
                client_unsupported: false,
            });
            continue;
        }

        let Some(path) = entry["path"].as_str() else {
            continue;
        };
        let path = normalize_name(path);
        if !accept(&path, install_mods, install_resourcepacks) {
            continue;
        }
        let relative = safe_relative(&path)?;
        let Some(api) = file_api.as_deref() else {
            return Err(ModpackError::Parse(
                "MCBBS pack has files but no fileApi URL".to_owned(),
            ));
        };
        files.push(PackFile {
            path: relative.to_string_lossy().replace('\\', "/"),
            size: entry["size"].as_u64().or_else(|| entry["fileSize"].as_u64()),
            sha1: entry["hash"].as_str().map(str::to_owned),
            sha512: None,
            urls: vec![format!("{api}/overrides/{}", encode_path(&path))],
            optional,
            client_unsupported: false,
        });
    }

    Ok(PackManifest {
        provider: Provider::Mcbbs,
        name: manifest["name"].as_str().unwrap_or("MCBBS pack").to_owned(),
        version: manifest["version"].as_str().unwrap_or("unknown").to_owned(),
        author: manifest["author"].as_str().unwrap_or_default().to_owned(),
        description: manifest["description"].as_str().unwrap_or_default().to_owned(),
        game_version,
        loader,
        files,
        overrides: vec!["overrides".to_owned()],
        manifest_name: "mcbbs.packmeta".to_owned(),
    })
}

fn accept(path: &str, install_mods: bool, install_resourcepacks: bool) -> bool {
    (install_mods && path.starts_with("mods/"))
        || (install_resourcepacks && path.starts_with("resourcepacks/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filters_by_category() {
        assert!(accept("mods/a.jar", true, false));
        assert!(!accept("mods/a.jar", false, true));
        assert!(accept("resourcepacks/a.zip", false, true));
        assert!(!accept("shaderpacks/a.zip", true, true));
    }
}
