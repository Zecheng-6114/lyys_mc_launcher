use serde_json::Value;

use crate::modpack::archive;
use crate::modpack::{LoaderKind, LoaderSpec, PackFile, PackManifest, Provider};

pub fn parse(index: &Value) -> Result<PackManifest, String> {
    let game_version = index["dependencies"]["minecraft"]
        .as_str()
        .ok_or_else(|| String::from("modrinth.index.json is missing dependencies.minecraft"))?
        .to_string();

    let loader = parse_loader(&index["dependencies"])?;

    let mut files = Vec::new();
    for entry in index["files"].as_array().into_iter().flatten() {
        let path = entry["path"]
            .as_str()
            .ok_or_else(|| String::from("modrinth file is missing path"))?;
        let path = normalize_path(path)?;

        let mut urls = Vec::new();
        for url in entry["downloads"].as_array().into_iter().flatten() {
            if let Some(url) = url.as_str().filter(|url| !url.is_empty()) {
                urls.push(url.to_string());
            }
        }
        if urls.is_empty() {
            return Err(format!("modrinth file has no download URL: {path}"));
        }

        let client = entry["env"]["client"].as_str().unwrap_or("required");
        let optional = client == "optional";
        let client_unsupported = client == "unsupported";

        files.push(PackFile {
            path,
            size: entry["fileSize"]
                .as_u64()
                .or_else(|| entry["size"].as_u64()),
            sha1: entry["hashes"]["sha1"].as_str().map(str::to_string),
            sha512: entry["hashes"]["sha512"].as_str().map(str::to_string),
            urls,
            optional,
            client_unsupported,
        });
    }

    Ok(PackManifest {
        provider: Provider::Modrinth,
        name: index["name"]
            .as_str()
            .unwrap_or("Modrinth pack")
            .to_string(),
        version: index["versionId"].as_str().unwrap_or("unknown").to_string(),
        author: String::new(),
        description: index["summary"].as_str().unwrap_or_default().to_string(),
        game_version,
        loader,
        files,
        overrides: vec![String::from("overrides"), String::from("client-overrides")],
        libraries: Vec::new(),
        manifest_name: String::from("modrinth.index.json"),
    })
}

fn parse_loader(dependencies: &Value) -> Result<Option<LoaderSpec>, String> {
    let mut found = None;

    for (key, kind) in [
        ("fabric-loader", LoaderKind::Fabric),
        ("forge", LoaderKind::Forge),
        ("neoforge", LoaderKind::NeoForge),
        ("neo-forge", LoaderKind::NeoForge),
    ] {
        if let Some(version) = dependencies[key].as_str() {
            if found.is_some() {
                return Err(String::from(
                    "modrinth.index.json declares more than one mod loader",
                ));
            }
            found = Some(LoaderSpec {
                kind,
                version: Some(version.to_string()),
            });
        }
    }

    if dependencies["quilt-loader"].as_str().is_some() {
        return Err(String::from(
            "Quilt Loader is not supported yet; only Fabric, Forge and NeoForge are supported",
        ));
    }

    Ok(found)
}

fn normalize_path(path: &str) -> Result<String, String> {
    let relative = archive::safe_relative(path)?;
    Ok(relative
        .components()
        .filter_map(|component| match component {
            std::path::Component::Normal(part) => Some(part.to_string_lossy().to_string()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_loader_files_and_optional_flags() {
        let index = serde_json::json!({
            "name": "Example",
            "versionId": "1.0.0",
            "summary": "test",
            "dependencies": {
                "minecraft": "1.20.1",
                "forge": "47.2.0"
            },
            "files": [
                {
                    "path": "mods/required.jar",
                    "hashes": {"sha1": "a", "sha512": "b"},
                    "downloads": ["https://example.invalid/required.jar"],
                    "fileSize": 12
                },
                {
                    "path": "mods/optional.jar",
                    "hashes": {"sha512": "c"},
                    "downloads": ["https://example.invalid/optional.jar"],
                    "env": {"client": "optional"}
                },
                {
                    "path": "mods/server.jar",
                    "hashes": {"sha512": "d"},
                    "downloads": ["https://example.invalid/server.jar"],
                    "env": {"client": "unsupported"}
                }
            ]
        });

        let pack = parse(&index).unwrap();
        assert_eq!(pack.game_version, "1.20.1");
        assert_eq!(pack.loader.as_ref().unwrap().kind, LoaderKind::Forge);
        assert_eq!(pack.files.len(), 3);
        assert!(pack.files[1].optional);
        assert!(pack.files[2].client_unsupported);
        assert_eq!(pack.overrides, ["overrides", "client-overrides"]);
    }

    #[test]
    fn rejects_quilt() {
        let index = serde_json::json!({
            "dependencies": {"minecraft": "1.20.1", "quilt-loader": "0.20.0"},
            "files": []
        });
        assert!(parse(&index).unwrap_err().contains("Quilt"));
    }

    #[test]
    fn rejects_path_traversal() {
        let index = serde_json::json!({
            "dependencies": {"minecraft": "1.20.1"},
            "files": [{
                "path": "../escape.jar",
                "hashes": {"sha512": "x"},
                "downloads": ["https://example.invalid/x.jar"]
            }]
        });
        assert!(parse(&index).is_err());
    }
}
