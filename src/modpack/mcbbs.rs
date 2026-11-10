use serde_json::Value;

use crate::modpack::archive;
use crate::modpack::curse::CurseClient;
use crate::modpack::{LoaderKind, LoaderSpec, PackFile, PackManifest, Provider};

pub fn parse(manifest: &Value, client: &mut CurseClient) -> Result<PackManifest, String> {
    let addons = manifest["addons"].as_array().cloned().unwrap_or_default();
    let mut game_version = None;
    let mut loader = None;

    for addon in &addons {
        let id = addon["id"].as_str().unwrap_or_default();
        let version = addon["version"].as_str().unwrap_or_default();
        match component_kind(id) {
            Some(ComponentKind::Game) => game_version = Some(version.to_string()),
            Some(ComponentKind::Fabric) => {
                loader = Some(LoaderSpec {
                    kind: LoaderKind::Fabric,
                    version: Some(version.to_string()),
                })
            }
            Some(ComponentKind::Forge) => {
                loader = Some(LoaderSpec {
                    kind: LoaderKind::Forge,
                    version: Some(version.to_string()),
                })
            }
            Some(ComponentKind::NeoForge) => {
                loader = Some(LoaderSpec {
                    kind: LoaderKind::NeoForge,
                    version: Some(version.to_string()),
                })
            }
            Some(ComponentKind::Quilt) => {
                return Err(String::from(
                    "Quilt Loader is not supported yet; only Fabric, Forge and NeoForge are supported",
                ));
            }
            None => {}
        }
    }

    let game_version =
        game_version.ok_or_else(|| String::from("MCBBS manifest has no game component"))?;

    let file_api = manifest["fileApi"]
        .as_str()
        .filter(|value| !value.is_empty())
        .map(|value| value.trim_end_matches('/').to_string());
    let install_mods = manifest["settings"]["install_mods"]
        .as_bool()
        .unwrap_or(true);
    let install_resourcepacks = manifest["settings"]["install_resourcepack"]
        .as_bool()
        .unwrap_or(true);

    let mut files = Vec::new();
    for file in manifest["files"].as_array().into_iter().flatten() {
        let file_type = file["type"].as_str().unwrap_or_default();
        match file_type {
            "addon" => {
                let Some(path) = file["path"].as_str() else {
                    continue;
                };
                if !accept(path, install_mods, install_resourcepacks) {
                    continue;
                }
                let path = archive::safe_relative(path)?
                    .to_string_lossy()
                    .replace('\\', "/");
                let Some(file_api) = &file_api else {
                    continue;
                };
                files.push(PackFile {
                    path: path.clone(),
                    size: None,
                    sha1: file["hash"].as_str().map(str::to_string),
                    sha512: None,
                    urls: vec![format!(
                        "{file_api}/overrides/{}",
                        crate::download::encode_path(&path)
                    )],
                    optional: false,
                    client_unsupported: false,
                });
            }
            "curse" => {
                let Some(project_id) = file["projectID"].as_i64() else {
                    continue;
                };
                let Some(file_id) = file["fileID"].as_i64() else {
                    continue;
                };
                let resolved = client.resolve_file(
                    project_id,
                    file_id,
                    file["fileName"].as_str(),
                    file["url"].as_str(),
                )?;
                if !accept(resolved.category(), install_mods, install_resourcepacks) {
                    continue;
                }
                let path = archive::safe_relative(&format!(
                    "{}/{}",
                    resolved.category(),
                    resolved.file_name
                ))?
                .to_string_lossy()
                .replace('\\', "/");
                files.push(PackFile {
                    path,
                    size: file["fileLength"].as_u64(),
                    sha1: resolved.sha1,
                    sha512: None,
                    urls: vec![resolved.url],
                    optional: false,
                    client_unsupported: false,
                });
            }
            _ => {}
        }
    }

    Ok(PackManifest {
        provider: Provider::Mcbbs,
        name: manifest["name"]
            .as_str()
            .unwrap_or("MCBBS pack")
            .to_string(),
        version: manifest["version"]
            .as_str()
            .unwrap_or("unknown")
            .to_string(),
        author: manifest["author"].as_str().unwrap_or_default().to_string(),
        description: manifest["description"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        game_version,
        loader,
        files,
        overrides: vec![String::from("overrides")],
        libraries: manifest["libraries"]
            .as_array()
            .cloned()
            .unwrap_or_default(),
        manifest_name: String::from("manifest.json"),
    })
}

enum ComponentKind {
    Game,
    Fabric,
    Forge,
    NeoForge,
    Quilt,
}

fn component_kind(id: &str) -> Option<ComponentKind> {
    let id = id.to_ascii_lowercase();
    if id == "game" || id == "net.minecraft" || id == "minecraft" {
        Some(ComponentKind::Game)
    } else if id == "fabric" || id.contains("fabric-loader") {
        Some(ComponentKind::Fabric)
    } else if id.contains("neoforge") {
        Some(ComponentKind::NeoForge)
    } else if id.contains("forge") {
        Some(ComponentKind::Forge)
    } else if id == "quilt" || id.contains("quilt-loader") {
        Some(ComponentKind::Quilt)
    } else {
        None
    }
}

fn accept(path: &str, install_mods: bool, install_resourcepacks: bool) -> bool {
    if (path == "mods" || path.starts_with("mods/")) && !install_mods {
        return false;
    }
    if (path == "resourcepacks" || path.starts_with("resourcepacks/")) && !install_resourcepacks {
        return false;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_addons_and_libraries() {
        let manifest = serde_json::json!({
            "manifestType": "minecraftModpack",
            "name": "MCBBS",
            "version": "1.0",
            "addons": [
                {"id": "game", "version": "1.20.1"},
                {"id": "forge", "version": "47.2.0"}
            ],
            "libraries": [
                {"name": "example:lib:1.0"}
            ],
            "files": []
        });

        let mut client = CurseClient::new();
        let pack = parse(&manifest, &mut client).unwrap();
        assert_eq!(pack.game_version, "1.20.1");
        assert_eq!(pack.loader.as_ref().unwrap().kind, LoaderKind::Forge);
        assert_eq!(pack.libraries.len(), 1);
    }
}
