use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::download::{self, Origins};
use crate::installer;
use crate::metadata;

const FABRIC_META: &str = "https://meta.fabricmc.net/v2";

pub fn install_fabric(
    mc_dir: &str,
    mc_version: &str,
    loader_version: Option<&str>,
    name: Option<&str>,
    origins: &Origins,
) -> Result<String, String> {
    let base = download::fetch_version_json_cached(mc_dir, mc_version, origins)?;

    let version = match loader_version {
        Some(v) => v.to_string(),
        None => latest_stable_loader(mc_version, origins)?,
    };

    let id = instance_name(mc_dir, name, &format!("{mc_version}_fabric_{version}"))?;
    install_fabric_at(mc_dir, mc_version, &version, &id, &base, origins)?;

    write_log(
        &id,
        &format!("loader: fabric {version} for minecraft {mc_version}\nversion files: ensured\n"),
    );

    Ok(id)
}

pub fn install_fabric_at(
    mc_dir: &str,
    mc_version: &str,
    version: &str,
    id: &str,
    base: &Value,
    origins: &Origins,
) -> Result<(), String> {
    let mut profile = download::fetch_json(
        &format!("{FABRIC_META}/versions/loader/{mc_version}/{version}/profile/json"),
        origins,
    )?;
    profile["id"] = Value::String(id.to_string());

    let dir = Path::new(mc_dir).join("versions").join(id);
    let result = (|| -> Result<(), String> {
        fs::create_dir_all(&dir)
            .map_err(|e| format!("failed to create dir {}: {e}", dir.display()))?;
        let text = serde_json::to_string_pretty(&profile).map_err(|e| e.to_string())?;
        fs::write(dir.join(format!("{id}.json")), text)
            .map_err(|e| format!("failed to write version.json: {e}"))?;

        download::ensure_version_files(mc_dir, base, id, origins)?;
        download::ensure_libraries(mc_dir, &profile, origins)?;
        crate::version::integrate(mc_dir, id, mc_version, base)
    })();

    if let Err(error) = result {
        let _ = fs::remove_dir_all(&dir);
        return Err(error);
    }

    Ok(())
}

pub fn install_forge(
    mc_dir: &str,
    mc_version: &str,
    loader_version: Option<&str>,
    name: Option<&str>,
    java: &str,
    origins: &Origins,
) -> Result<String, String> {
    let installer = metadata::forge_installer(mc_version, loader_version, origins)?;
    install_loader(mc_dir, mc_version, name, "forge", &installer, java, origins)
}

pub fn install_forge_at(
    mc_dir: &str,
    mc_version: &str,
    loader_version: Option<&str>,
    id: &str,
    java: &str,
    origins: &Origins,
) -> Result<(), String> {
    let installer = metadata::forge_installer(mc_version, loader_version, origins)?;
    install_loader_at(mc_dir, mc_version, id, "forge", &installer, java, origins)
}

pub fn install_neoforge(
    mc_dir: &str,
    mc_version: &str,
    loader_version: Option<&str>,
    name: Option<&str>,
    java: &str,
    origins: &Origins,
) -> Result<String, String> {
    let installer = metadata::neoforge_installer(mc_version, loader_version, origins)?;
    install_loader(
        mc_dir, mc_version, name, "neoforge", &installer, java, origins,
    )
}

pub fn install_neoforge_at(
    mc_dir: &str,
    mc_version: &str,
    loader_version: Option<&str>,
    id: &str,
    java: &str,
    origins: &Origins,
) -> Result<(), String> {
    let installer = metadata::neoforge_installer(mc_version, loader_version, origins)?;
    install_loader_at(
        mc_dir, mc_version, id, "neoforge", &installer, java, origins,
    )
}

fn install_loader(
    mc_dir: &str,
    mc_version: &str,
    name: Option<&str>,
    kind: &str,
    remote: &metadata::LoaderInstaller,
    java: &str,
    origins: &Origins,
) -> Result<String, String> {
    let id = instance_name(
        mc_dir,
        name,
        &format!("{mc_version}_{kind}_{}", remote.display_version),
    )?;
    install_loader_at(mc_dir, mc_version, &id, kind, remote, java, origins)?;
    Ok(id)
}

fn install_loader_at(
    mc_dir: &str,
    mc_version: &str,
    id: &str,
    kind: &str,
    remote: &metadata::LoaderInstaller,
    java: &str,
    origins: &Origins,
) -> Result<(), String> {
    let versions = Path::new(mc_dir).join("versions").join(id);
    if versions.exists() {
        return Err(format!("instance already exists: {id}"));
    }

    let base = download::fetch_version_json_cached(mc_dir, mc_version, origins)?;

    let cache = Path::new(mc_dir).join(".cache");
    fs::create_dir_all(&cache)
        .map_err(|e| format!("failed to create dir {}: {e}", cache.display()))?;

    let installer_path = cache.join(format!(
        "installer-{kind}-{}.jar",
        safe_name(&remote.version)
    ));
    if installer_path.is_file() && installer::validate_zip(&installer_path).is_err() {
        let _ = fs::remove_file(&installer_path);
    }
    if !installer_path.is_file() {
        let mut last_error = String::new();
        for url in &remote.urls {
            match download::fetch(url, &installer_path, None, origins) {
                Ok(()) => {
                    last_error.clear();
                    break;
                }
                Err(error) => {
                    last_error = error;
                    let _ = fs::remove_file(&installer_path);
                }
            }
        }
        if !last_error.is_empty() {
            return Err(last_error);
        }
        installer::validate_zip(&installer_path)?;
    }

    let info = installer::inspect(&installer_path)?;
    if info.needs_java {
        crate::launch::check_java(java, &base)?;
    }
    if let Some(profile_mc) = installer::profile_minecraft(&info.profile)
        && profile_mc != mc_version
    {
        return Err(format!(
            "installer targets Minecraft {profile_mc}, but {mc_version} was requested"
        ));
    }

    download::ensure_version_files(mc_dir, &base, id, origins)?;

    let result = (if info.new_format {
        installer::install_new(
            mc_dir,
            id,
            mc_version,
            &installer_path,
            &info,
            java,
            origins,
        )
    } else {
        installer::install_legacy(mc_dir, id, mc_version, &installer_path, &info, origins)
    })
    .and_then(|()| crate::version::integrate(mc_dir, id, mc_version, &base));

    match result {
        Ok(()) => {
            write_log(
                id,
                &format!(
                    "loader: {kind} {} for minecraft {mc_version}\ninstaller: {}\nformat: {}\n",
                    remote.version,
                    installer_path.display(),
                    if info.new_format {
                        "processors"
                    } else {
                        "legacy"
                    }
                ),
            );
            Ok(())
        }
        Err(error) => {
            let _ = fs::remove_dir_all(&versions);
            Err(error)
        }
    }
}

fn latest_stable_loader(mc_version: &str, origins: &Origins) -> Result<String, String> {
    let list = match download::fetch_json(
        &format!("{FABRIC_META}/versions/loader/{mc_version}"),
        origins,
    ) {
        Ok(list) => list,
        Err(error) if download::is_client_error(&error) => {
            return Err(format!(
                "Fabric Loader does not support Minecraft {mc_version}"
            ));
        }
        Err(error) => return Err(error),
    };
    let items = list.as_array().cloned().unwrap_or_default();

    items
        .iter()
        .find(|item| item["loader"]["stable"].as_bool() == Some(true))
        .or_else(|| items.first())
        .and_then(|item| item["loader"]["version"].as_str())
        .map(|version| version.to_string())
        .ok_or_else(|| format!("no Fabric Loader for {mc_version}"))
}

pub fn instance_name(mc_dir: &str, name: Option<&str>, base: &str) -> Result<String, String> {
    match name {
        Some(name) => {
            validate_instance_id(name)?;
            let dir = Path::new(mc_dir).join("versions").join(name);
            if dir.exists() {
                return Err(format!("instance already exists: {name}"));
            }
            Ok(name.to_string())
        }
        None => Ok(instance_id(mc_dir, base)),
    }
}

pub fn validate_instance_id(id: &str) -> Result<(), String> {
    if id.trim().is_empty() {
        return Err(String::from("instance name must not be empty"));
    }
    if id != id.trim() {
        return Err(format!(
            "invalid instance name: {id} (leading or trailing spaces)"
        ));
    }
    if id == "." || id == ".." {
        return Err(format!("invalid instance name: {id}"));
    }
    if id.chars().any(|c| {
        c.is_control() || matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|')
    }) {
        return Err(format!(
            "invalid instance name: {id} (path separators and reserved characters are not allowed)"
        ));
    }
    Ok(())
}

fn instance_id(mc_dir: &str, base: &str) -> String {
    let versions = Path::new(mc_dir).join("versions");
    let base = safe_name(base);
    let mut candidate = base.clone();
    let mut number = 1;

    while versions.join(&candidate).exists() {
        candidate = format!("{base}_{number}");
        number += 1;
    }

    candidate
}

pub fn safe_name(name: &str) -> String {
    let mut out: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect();

    if out.is_empty() {
        out.push_str("instance");
    }
    out
}

pub fn base_dir() -> PathBuf {
    if let Some(dir) = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
    {
        let mut cursor = Some(dir.as_path());
        while let Some(current) = cursor {
            if current.join("Cargo.toml").exists() {
                return current.to_path_buf();
            }
            cursor = current.parent();
        }
    }

    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

fn log_dir() -> Result<PathBuf, String> {
    let dir = base_dir().join("lyys_mc_launcher").join("log");
    fs::create_dir_all(&dir).map_err(|e| format!("failed to create dir {}: {e}", dir.display()))?;
    Ok(dir)
}

fn log_path(id: &str) -> Result<PathBuf, String> {
    Ok(log_dir()?.join(format!("install-{}.log", safe_name(id))))
}

fn write_log(id: &str, text: &str) {
    if let Ok(path) = log_path(id)
        && let Ok(mut file) = fs::OpenOptions::new().create(true).append(true).open(&path)
    {
        let _ = writeln!(file, "{text}");
        println!("install log: {}", path.display());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_instance_names() {
        assert!(validate_instance_id("1.20.1_forge_47.2.0").is_ok());
        assert!(validate_instance_id("").is_err());
        assert!(validate_instance_id("../escape").is_err());
        assert!(validate_instance_id("a/b").is_err());
    }

    #[test]
    fn safe_name_replaces_reserved_characters() {
        assert_eq!(safe_name("1.20.1:forge"), "1.20.1_forge");
        assert_eq!(safe_name("涟漪之篇·如涟漪之所见"), "涟漪之篇_如涟漪之所见");
        assert_eq!(safe_name(""), "instance");
    }
}
