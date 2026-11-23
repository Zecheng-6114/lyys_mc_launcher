//! Modrinth 整合包（`modrinth.index.json`）。

use serde_json::Value;

use super::{ModpackError, PackFile, PackManifest, Provider, safe_relative};

pub fn parse(index: &Value) -> Result<PackManifest, ModpackError> {
    let game_version = index["dependencies"]["minecraft"]
        .as_str()
        .filter(|version| !version.trim().is_empty())
        .ok_or_else(|| ModpackError::Parse("modrinth.index.json has no Minecraft version".to_owned()))?
        .to_owned();
    let loader = parse_loader(&index["dependencies"])?;

    let mut files = Vec::new();
    for entry in index["files"].as_array().into_iter().flatten() {
        let Some(path) = entry["path"].as_str() else {
            continue;
        };
        let path = safe_relative(path)?
            .to_string_lossy()
            .replace('\\', "/");
        let urls = entry["downloads"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect::<Vec<_>>();
        if urls.is_empty() {
            return Err(ModpackError::Parse(format!(
                "modrinth.index.json file {path} has no download URL"
            )));
        }
        let client = entry["env"]["client"].as_str().unwrap_or("required");
        files.push(PackFile {
            path,
            size: entry["fileSize"]
                .as_u64()
                .or_else(|| entry["size"].as_u64()),
            sha1: entry["hashes"]["sha1"].as_str().map(str::to_owned),
            sha512: entry["hashes"]["sha512"].as_str().map(str::to_owned),
            urls,
            optional: client == "optional",
            client_unsupported: client == "unsupported",
        });
    }

    Ok(PackManifest {
        provider: Provider::Modrinth,
        name: index["name"].as_str().unwrap_or("Modrinth pack").to_owned(),
        version: index["versionId"].as_str().unwrap_or("unknown").to_owned(),
        author: String::new(),
        description: index["summary"].as_str().unwrap_or_default().to_owned(),
        game_version,
        loader,
        files,
        overrides: vec!["overrides".to_owned(), "client-overrides".to_owned()],
        manifest_name: "modrinth.index.json".to_owned(),
    })
}

fn parse_loader(dependencies: &Value) -> Result<Option<mc_launcher_core::loader::common::LoaderSpec>, ModpackError> {
    let mut found = None;
    for (key, kind) in [
        ("fabric-loader", "fabric"),
        ("forge", "forge"),
        ("neoforge", "neoforge"),
        ("neo-forge", "neoforge"),
        ("quilt-loader", "quilt"),
    ] {
        let Some(version) = dependencies[key].as_str().filter(|version| !version.trim().is_empty())
        else {
            continue;
        };
        if found.is_some() {
            return Err(ModpackError::Parse(
                "modrinth.index.json declares more than one mod loader".to_owned(),
            ));
        }
        found = super::loader_spec(kind, version.trim());
    }
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_files_and_loader() {
        let index = serde_json::json!({
            "name": "Example",
            "versionId": "1.0.0",
            "dependencies": {"minecraft": "1.20.1", "fabric-loader": "0.15.0"},
            "files": [
                {"path": "mods/a.jar", "downloads": ["https://example.invalid/a.jar"],
                 "hashes": {"sha1": "abc", "sha512": "def"}, "fileSize": 10,
                 "env": {"client": "required"}},
                {"path": "mods/b.jar", "downloads": ["https://example.invalid/b.jar"],
                 "env": {"client": "unsupported"}}
            ]
        });
        let pack = parse(&index).unwrap();
        assert_eq!(pack.game_version, "1.20.1");
        assert!(pack.loader.is_some());
        assert_eq!(pack.files.len(), 2);
        assert!(pack.files[1].client_unsupported);
        assert_eq!(pack.files[0].path, "mods/a.jar");
        assert_eq!(pack.installable_files(true), 1);
    }
}
