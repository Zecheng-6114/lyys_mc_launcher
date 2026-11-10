use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

use serde_json::Value;

use crate::modpack::Provider;

pub struct Detected {
    pub provider: Provider,
    pub prefix: String,
    pub manifest_name: String,
}

pub fn detect(path: &Path) -> Result<Detected, String> {
    let file =
        fs::File::open(path).map_err(|e| format!("failed to open {}: {e}", path.display()))?;
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|e| format!("failed to read {}: {e}", path.display()))?;

    let names = entry_names(&mut archive)?;
    let name_set: std::collections::HashSet<&str> = names.iter().map(String::as_str).collect();
    let mut prefixes = vec![String::new()];
    let mut nested = std::collections::BTreeSet::new();
    for name in &names {
        if let Some((top, _)) = name.split_once('/') {
            nested.insert(format!("{top}/"));
        }
    }
    prefixes.extend(nested);

    for prefix in prefixes {
        let has = |name: &str| name_set.contains(format!("{prefix}{name}").as_str());
        if has("modrinth.index.json") {
            return Ok(Detected {
                provider: Provider::Modrinth,
                prefix,
                manifest_name: String::from("modrinth.index.json"),
            });
        }
        if has("mcbbs.packmeta") {
            return Ok(Detected {
                provider: Provider::Mcbbs,
                prefix,
                manifest_name: String::from("mcbbs.packmeta"),
            });
        }
        if has("server-manifest.json") {
            return Ok(Detected {
                provider: Provider::Server,
                prefix,
                manifest_name: String::from("server-manifest.json"),
            });
        }
        if has("manifest.json")
            && let Some(text) = read_text(&mut archive, &format!("{prefix}manifest.json"))?
            && let Ok(json) = serde_json::from_str::<Value>(&text)
        {
            if json["minecraft"].is_object() {
                return Ok(Detected {
                    provider: Provider::CurseForge,
                    prefix,
                    manifest_name: String::from("manifest.json"),
                });
            }
            if json.get("addons").is_some() || json.get("launchInfo").is_some() {
                return Ok(Detected {
                    provider: Provider::Mcbbs,
                    prefix,
                    manifest_name: String::from("manifest.json"),
                });
            }
        }
        if has("mmc-pack.json") && has("instance.cfg") {
            return Ok(Detected {
                provider: Provider::MultiMc,
                prefix,
                manifest_name: String::from("mmc-pack.json"),
            });
        }
    }

    Err(format!(
        "{} is not a supported modpack (expected modrinth.index.json, manifest.json, mcbbs.packmeta, server-manifest.json or mmc-pack.json)",
        path.display()
    ))
}

pub fn read_entry_text(
    archive: &mut zip::ZipArchive<fs::File>,
    name: &str,
) -> Result<Option<String>, String> {
    read_text(archive, name)
}

fn read_text(
    archive: &mut zip::ZipArchive<fs::File>,
    name: &str,
) -> Result<Option<String>, String> {
    let Some(index) = find_index(archive, name)? else {
        return Ok(None);
    };
    let mut entry = archive
        .by_index(index)
        .map_err(|e| format!("failed to read {name}: {e}"))?;
    let mut text = String::new();
    entry
        .read_to_string(&mut text)
        .map_err(|e| format!("failed to read {name}: {e}"))?;
    Ok(Some(text.trim_start_matches('\u{feff}').to_string()))
}

pub fn copy_entry(path: &Path, prefix: &str, name: &str, dest: &Path) -> Result<bool, String> {
    let file =
        fs::File::open(path).map_err(|e| format!("failed to open {}: {e}", path.display()))?;
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|e| format!("failed to read {}: {e}", path.display()))?;
    let full = format!("{prefix}{name}");
    let Some(index) = find_index(&mut archive, &full)? else {
        return Ok(false);
    };
    let mut entry = archive
        .by_index(index)
        .map_err(|e| format!("failed to read {full}: {e}"))?;

    let mut bytes = Vec::new();
    entry
        .read_to_end(&mut bytes)
        .map_err(|e| format!("failed to read {full}: {e}"))?;
    if let Some(dir) = dest.parent() {
        fs::create_dir_all(dir).map_err(|e| format!("failed to create {}: {e}", dir.display()))?;
    }
    fs::write(dest, bytes).map_err(|e| format!("failed to write {}: {e}", dest.display()))?;
    Ok(true)
}

pub fn extract_overrides(
    path: &Path,
    prefix: &str,
    dirs: &[String],
    root: &Path,
) -> Result<usize, String> {
    let mut written = 0;

    for dir in dirs {
        let dir = dir.trim_matches('/').replace('\\', "/");
        if dir.is_empty() {
            continue;
        }
        let dir_prefix = format!("{prefix}{dir}/");

        let file =
            fs::File::open(path).map_err(|e| format!("failed to open {}: {e}", path.display()))?;
        let mut archive = zip::ZipArchive::new(file)
            .map_err(|e| format!("failed to read {}: {e}", path.display()))?;

        for index in 0..archive.len() {
            let mut entry = archive
                .by_index(index)
                .map_err(|e| format!("failed to read {}: {e}", path.display()))?;
            if entry.is_dir() {
                continue;
            }

            let name = entry.name().replace('\\', "/");
            let Some(relative) = name.strip_prefix(&dir_prefix) else {
                continue;
            };
            let relative = safe_relative(relative)?;
            let dest = root.join(relative);
            if let Some(parent) = dest.parent() {
                fs::create_dir_all(parent)
                    .map_err(|e| format!("failed to create {}: {e}", parent.display()))?;
            }

            let mut bytes = Vec::new();
            entry
                .read_to_end(&mut bytes)
                .map_err(|e| format!("failed to read {name}: {e}"))?;
            fs::write(&dest, bytes)
                .map_err(|e| format!("failed to write {}: {e}", dest.display()))?;
            written += 1;
        }
    }

    Ok(written)
}

pub fn safe_relative(name: &str) -> Result<PathBuf, String> {
    let normalized = name.replace('\\', "/");
    if normalized.is_empty() {
        return Err(String::from("empty archive path"));
    }
    if normalized.as_bytes().get(1) == Some(&b':') {
        return Err(format!("absolute path is not allowed: {name}"));
    }

    let path = Path::new(&normalized);
    if path.is_absolute() {
        return Err(format!("absolute path is not allowed: {name}"));
    }

    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => out.push(part),
            Component::CurDir => {}
            _ => return Err(format!("unsafe archive path: {name}")),
        }
    }

    if out.as_os_str().is_empty() {
        Err(format!("unsafe archive path: {name}"))
    } else {
        Ok(out)
    }
}

fn entry_names(archive: &mut zip::ZipArchive<fs::File>) -> Result<Vec<String>, String> {
    let mut names = Vec::with_capacity(archive.len());
    for index in 0..archive.len() {
        let entry = archive
            .by_index(index)
            .map_err(|e| format!("failed to read archive: {e}"))?;
        names.push(entry.name().replace('\\', "/"));
    }
    Ok(names)
}

fn find_index(
    archive: &mut zip::ZipArchive<fs::File>,
    name: &str,
) -> Result<Option<usize>, String> {
    let name = name.replace('\\', "/");
    for index in 0..archive.len() {
        let entry = archive
            .by_index(index)
            .map_err(|e| format!("failed to read archive: {e}"))?;
        if entry.name().replace('\\', "/") == name {
            return Ok(Some(index));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_path(name: &str) -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default();
        std::env::temp_dir().join(format!("lyys-archive-{name}-{stamp}.zip"))
    }

    fn write_zip(path: &Path, files: &[(&str, &str)]) {
        let file = fs::File::create(path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default();
        for (name, content) in files {
            zip.start_file(*name, options).unwrap();
            zip.write_all(content.as_bytes()).unwrap();
        }
        zip.finish().unwrap();
    }

    #[test]
    fn rejects_unsafe_paths() {
        assert!(safe_relative("../escape").is_err());
        assert!(safe_relative("/absolute").is_err());
        assert!(safe_relative(r"C:\absolute").is_err());
        assert_eq!(
            safe_relative("mods/example.jar").unwrap(),
            PathBuf::from("mods").join("example.jar")
        );
    }

    #[test]
    fn detects_modrinth_and_nested_roots() {
        let path = temp_path("mrpack");
        write_zip(
            &path,
            &[
                ("pack/modrinth.index.json", "{}"),
                ("pack/overrides/config/a.txt", "a"),
            ],
        );

        let detected = detect(&path).unwrap();
        assert_eq!(detected.provider, Provider::Modrinth);
        assert_eq!(detected.prefix, "pack/");

        let _ = fs::remove_file(path);
    }

    #[test]
    fn extracts_backslash_entry_names() {
        let path = temp_path("backslash");
        write_zip(&path, &[("overrides\\config\\a.txt", "a")]);

        let root = path.with_extension("backslash-out");
        let _ = fs::remove_dir_all(&root);
        let count = extract_overrides(&path, "", &[String::from("overrides")], &root).unwrap();
        assert_eq!(count, 1);
        assert_eq!(fs::read_to_string(root.join("config/a.txt")).unwrap(), "a");

        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn extracts_overrides_with_later_directories_winning() {
        let path = temp_path("overrides");
        write_zip(
            &path,
            &[
                ("overrides/config/a.txt", "base"),
                ("client-overrides/config/a.txt", "client"),
            ],
        );

        let root = path.with_extension("out");
        let _ = fs::remove_dir_all(&root);
        let count = extract_overrides(
            &path,
            "",
            &[String::from("overrides"), String::from("client-overrides")],
            &root,
        )
        .unwrap();
        assert_eq!(count, 2);
        assert_eq!(
            fs::read_to_string(root.join("config/a.txt")).unwrap(),
            "client"
        );

        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_file(path);
    }
}
