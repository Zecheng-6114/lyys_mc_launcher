use serde_json::Value;

use crate::modpack::archive;
use crate::modpack::{LoaderKind, LoaderSpec, PackFile, PackManifest, Provider};

pub fn parse(manifest: &Value) -> Result<PackManifest, String> {
    let addons = manifest["addons"].as_array().cloned().unwrap_or_default();
    let mut game_version = None;
    let mut loader = None;

    for addon in &addons {
        match addon["id"].as_str().unwrap_or_default() {
            "game" | "minecraft" | "net.minecraft" => {
                game_version = Some(addon["version"].as_str().unwrap_or_default().to_string());
            }
            "fabric" | "net.fabricmc.fabric-loader" => {
                loader = Some(LoaderSpec {
                    kind: LoaderKind::Fabric,
                    version: Some(addon["version"].as_str().unwrap_or_default().to_string()),
                });
            }
            "forge" | "net.minecraftforge" => {
                loader = Some(LoaderSpec {
                    kind: LoaderKind::Forge,
                    version: Some(addon["version"].as_str().unwrap_or_default().to_string()),
                });
            }
            "neoforge" | "net.neoforged" => {
                loader = Some(LoaderSpec {
                    kind: LoaderKind::NeoForge,
                    version: Some(addon["version"].as_str().unwrap_or_default().to_string()),
                });
            }
            "quilt" | "org.quiltmc.quilt-loader" => {
                return Err(String::from(
                    "Quilt Loader is not supported yet; only Fabric, Forge and NeoForge are supported",
                ));
            }
            _ => {}
        }
    }

    let game_version =
        game_version.ok_or_else(|| String::from("Server manifest has no game component"))?;
    let file_api = manifest["fileApi"]
        .as_str()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| String::from("Server manifest has no fileApi"))?
        .trim_end_matches('/')
        .to_string();

    let mut files = Vec::new();
    for file in manifest["files"].as_array().into_iter().flatten() {
        let path = file["path"]
            .as_str()
            .ok_or_else(|| String::from("Server manifest file has no path"))?;
        let path = archive::safe_relative(path)?
            .to_string_lossy()
            .replace('\\', "/");
        files.push(PackFile {
            urls: vec![format!(
                "{file_api}/overrides/{}",
                crate::download::encode_path(&path)
            )],
            path,
            size: file["size"].as_u64(),
            sha1: file["hash"].as_str().map(str::to_string),
            sha512: None,
            optional: false,
            client_unsupported: false,
        });
    }

    Ok(PackManifest {
        provider: Provider::Server,
        name: manifest["name"]
            .as_str()
            .unwrap_or("Server pack")
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
        libraries: Vec::new(),
        manifest_name: String::from("server-manifest.json"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_server_files() {
        let manifest = serde_json::json!({
            "name": "Server",
            "version": "1.0",
            "fileApi": "https://example.invalid/files/",
            "addons": [
                {"id": "game", "version": "1.20.1"},
                {"id": "fabric", "version": "0.15.0"}
            ],
            "files": [
                {"path": "mods/a.jar", "hash": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "size": 5}
            ]
        });

        let pack = parse(&manifest).unwrap();
        assert_eq!(pack.game_version, "1.20.1");
        assert_eq!(pack.loader.as_ref().unwrap().kind, LoaderKind::Fabric);
        assert_eq!(
            pack.files[0].urls[0],
            "https://example.invalid/files/overrides/mods/a.jar"
        );
    }
}
