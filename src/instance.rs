use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

pub const METADATA_FILE: &str = "lyys-instance.json";
pub const LAUNCH_FILE: &str = "launch.json";

#[derive(Default, Clone)]
pub struct LaunchConfig {
    pub username: Option<String>,
    pub memory: Option<u32>,
    pub java: Option<String>,
    pub mirror: Option<String>,
    pub maven: Option<String>,
    pub isolate: Option<bool>,
}

pub struct ModpackRecord<'a> {
    pub provider: &'a str,
    pub name: &'a str,
    pub version: &'a str,
    pub source: &'a str,
    pub manifest: &'a str,
    pub excluded: &'a [String],
}

pub fn metadata_path(mc_dir: &str, instance: &str) -> PathBuf {
    Path::new(mc_dir)
        .join("versions")
        .join(instance)
        .join(METADATA_FILE)
}

pub fn read_metadata(mc_dir: &str, instance: &str) -> Option<Value> {
    let text = fs::read_to_string(metadata_path(mc_dir, instance)).ok()?;
    serde_json::from_str(&text).ok()
}

pub fn is_isolated(mc_dir: &str, instance: &str) -> bool {
    read_launch_config(mc_dir, instance)
        .isolate
        .or_else(|| {
            read_metadata(mc_dir, instance).and_then(|metadata| metadata["isolate"].as_bool())
        })
        .unwrap_or(false)
}

pub fn launch_path(mc_dir: &str, instance: &str) -> PathBuf {
    Path::new(mc_dir)
        .join("versions")
        .join(instance)
        .join(LAUNCH_FILE)
}

pub fn read_launch_config(mc_dir: &str, instance: &str) -> LaunchConfig {
    let Ok(text) = fs::read_to_string(launch_path(mc_dir, instance)) else {
        return LaunchConfig::default();
    };
    let Ok(value) = serde_json::from_str::<Value>(&text) else {
        return LaunchConfig::default();
    };

    LaunchConfig {
        username: non_empty_string(&value, "username"),
        memory: value["memory"]
            .as_u64()
            .and_then(|value| u32::try_from(value).ok()),
        java: non_empty_string(&value, "java"),
        mirror: non_empty_string(&value, "mirror"),
        maven: non_empty_string(&value, "maven"),
        isolate: value["isolate"].as_bool(),
    }
}

pub fn write_launch_config(
    mc_dir: &str,
    instance: &str,
    config: &LaunchConfig,
) -> Result<(), String> {
    let path = launch_path(mc_dir, instance);
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| format!("failed to create {}: {e}", dir.display()))?;
    }

    let mut value = serde_json::Map::new();
    if let Some(username) = &config.username {
        value.insert(String::from("username"), Value::String(username.clone()));
    }
    if let Some(memory) = config.memory {
        value.insert(String::from("memory"), Value::Number(memory.into()));
    }
    if let Some(java) = &config.java {
        value.insert(String::from("java"), Value::String(java.clone()));
    }
    if let Some(mirror) = &config.mirror {
        value.insert(String::from("mirror"), Value::String(mirror.clone()));
    }
    if let Some(maven) = &config.maven {
        value.insert(String::from("maven"), Value::String(maven.clone()));
    }
    if let Some(isolate) = config.isolate {
        value.insert(String::from("isolate"), Value::Bool(isolate));
    }

    let text = serde_json::to_string_pretty(&Value::Object(value)).map_err(|e| e.to_string())?;
    fs::write(&path, text).map_err(|e| format!("failed to write {}: {e}", path.display()))
}

fn non_empty_string(value: &Value, key: &str) -> Option<String> {
    value[key]
        .as_str()
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

pub fn game_dir(mc_dir: &str, instance: &str, isolate: bool) -> String {
    if isolate || is_isolated(mc_dir, instance) {
        format!("{mc_dir}/versions/{instance}")
    } else {
        mc_dir.to_string()
    }
}

pub fn write_metadata(
    mc_dir: &str,
    instance: &str,
    isolate: bool,
    modpack: Option<&ModpackRecord<'_>>,
) -> Result<(), String> {
    let dir = Path::new(mc_dir).join("versions").join(instance);
    fs::create_dir_all(&dir).map_err(|e| format!("failed to create {}: {e}", dir.display()))?;

    let modpack = match modpack {
        Some(record) => json!({
            "provider": record.provider,
            "name": record.name,
            "version": record.version,
            "source": record.source,
            "manifest": record.manifest,
            "excluded": record.excluded,
        }),
        None => Value::Null,
    };

    let metadata = json!({
        "format": 1,
        "isolate": isolate,
        "modpack": modpack,
    });
    let text = serde_json::to_string_pretty(&metadata).map_err(|e| e.to_string())?;
    fs::write(metadata_path(mc_dir, instance), text)
        .map_err(|e| format!("failed to write instance metadata: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_mc_dir(name: &str) -> String {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default();
        let dir = std::env::temp_dir().join(format!("lyys-instance-{name}-{stamp}"));
        std::fs::create_dir_all(&dir).unwrap();
        dir.to_string_lossy().to_string()
    }

    #[test]
    fn metadata_roundtrip_and_isolation() {
        let mc_dir = temp_mc_dir("roundtrip");
        let excluded = vec![String::from("mods/optional.jar")];
        write_metadata(
            &mc_dir,
            "pack",
            true,
            Some(&ModpackRecord {
                provider: "modrinth",
                name: "Pack",
                version: "1.0",
                source: "pack.mrpack",
                manifest: "modrinth.index.json",
                excluded: &excluded,
            }),
        )
        .unwrap();

        assert!(is_isolated(&mc_dir, "pack"));
        assert_eq!(
            game_dir(&mc_dir, "pack", false),
            format!("{mc_dir}/versions/pack")
        );
        let metadata = read_metadata(&mc_dir, "pack").unwrap();
        assert_eq!(metadata["modpack"]["provider"], "modrinth");

        let _ = fs::remove_dir_all(&mc_dir);
    }
}
