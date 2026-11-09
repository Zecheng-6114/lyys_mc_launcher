use serde_json::Value;

use crate::download::{self, Origins};

const FORGE_MAVEN: &str = "https://maven.minecraftforge.net/net/minecraftforge/forge";
const NEOFORGE_OLD_MAVEN: &str = "https://maven.neoforged.net/releases/net/neoforged/forge";
const NEOFORGE_MAVEN: &str = "https://maven.neoforged.net/releases/net/neoforged/neoforge";
const NEOFORGE_OLD_MIRROR: &str = "https://neoforged.forgecdn.net/releases/net/neoforged/forge";
const NEOFORGE_MIRROR: &str = "https://neoforged.forgecdn.net/releases/net/neoforged/neoforge";

pub struct LoaderInstaller {
    pub version: String,
    pub display_version: String,
    pub urls: Vec<String>,
}

pub fn forge_installer(
    mc_version: &str,
    requested: Option<&str>,
    origins: &Origins,
) -> Result<LoaderInstaller, String> {
    let requested = requested.map(|version| {
        let version = normalize_forge_version(version);
        version
            .strip_prefix(&format!("{mc_version}-"))
            .unwrap_or(&version)
            .to_string()
    });
    let versions = fetch_versions(&format!("{FORGE_MAVEN}/maven-metadata.xml"), origins).ok();

    let (loader_version, artifact) = match requested {
        Some(loader_version) => {
            let artifact = match &versions {
                Some(versions) => versions
                    .iter()
                    .filter(|candidate| {
                        forge_artifact_matches(candidate, mc_version, &loader_version)
                    })
                    .max_by(|a, b| compare_versions(a, b))
                    .cloned()
                    .unwrap_or_else(|| forge_artifact(mc_version, &loader_version)),
                None => forge_artifact(mc_version, &loader_version),
            };

            if let Some(versions) = &versions
                && !versions.iter().any(|candidate| candidate == &artifact)
            {
                let available: Vec<&String> = versions
                    .iter()
                    .filter(|candidate| candidate.starts_with(&format!("{mc_version}-")))
                    .collect();
                if available.is_empty() {
                    return Err(format!("Forge does not support Minecraft {mc_version}"));
                }
                return Err(format!(
                    "Forge {loader_version} is not available for Minecraft {mc_version} (try: {})",
                    available
                        .iter()
                        .rev()
                        .take(5)
                        .map(|value| value.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }

            (loader_version, artifact)
        }
        None => {
            let versions = versions.ok_or_else(|| {
                format!("failed to fetch Forge version list for Minecraft {mc_version}")
            })?;
            let artifact = versions
                .iter()
                .filter(|candidate| candidate.starts_with(&format!("{mc_version}-")))
                .max_by(|a, b| compare_versions(a, b))
                .cloned()
                .ok_or_else(|| format!("Forge does not support Minecraft {mc_version}"))?;
            let loader = forge_loader_from_artifact(&artifact, mc_version);
            (loader, artifact)
        }
    };

    Ok(LoaderInstaller {
        version: loader_version.clone(),
        display_version: loader_version,
        urls: vec![format!(
            "{FORGE_MAVEN}/{artifact}/forge-{artifact}-installer.jar"
        )],
    })
}

fn forge_loader_from_artifact(artifact: &str, mc_version: &str) -> String {
    let rest = artifact
        .strip_prefix(&format!("{mc_version}-"))
        .unwrap_or(artifact);
    rest.strip_suffix(&format!("-{mc_version}"))
        .unwrap_or(rest)
        .to_string()
}

pub fn neoforge_installer(
    mc_version: &str,
    requested: Option<&str>,
    origins: &Origins,
) -> Result<LoaderInstaller, String> {
    let old = mc_version == "1.20.1";
    let (maven, mirror, prefix, artifact_name) = if old {
        (
            NEOFORGE_OLD_MAVEN,
            NEOFORGE_OLD_MIRROR,
            String::from("1.20.1-"),
            "forge",
        )
    } else {
        (
            NEOFORGE_MAVEN,
            NEOFORGE_MIRROR,
            neoforge_prefix(mc_version)?,
            "neoforge",
        )
    };

    let versions = fetch_neoforge_versions(maven, mirror, origins).ok();
    let version = match requested {
        Some(requested) => normalize_neoforge_version(requested, &prefix),
        None => match &versions {
            Some(versions) => latest_neoforge_version(versions, &prefix)
                .ok_or_else(|| format!("no NeoForge version for Minecraft {mc_version}"))?,
            None => {
                return Err(format!(
                    "failed to fetch NeoForge version list for Minecraft {mc_version}"
                ));
            }
        },
    };

    if !neoforge_version_matches(&version, &prefix) {
        return Err(format!("invalid NeoForge version: {version}"));
    }

    if let Some(versions) = &versions
        && !versions.iter().any(|candidate| candidate == &version)
    {
        let available: Vec<&String> = versions
            .iter()
            .filter(|candidate| neoforge_version_matches(candidate, &prefix))
            .collect();
        return Err(format!(
            "NeoForge {version} is not available for Minecraft {mc_version} (try: {})",
            available
                .iter()
                .rev()
                .take(5)
                .map(|value| value.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }

    let display = if old {
        version
            .strip_prefix(&prefix)
            .unwrap_or(&version)
            .trim_start_matches('-')
            .to_string()
    } else {
        version.clone()
    };

    let file = format!("{artifact_name}-{version}-installer.jar");
    Ok(LoaderInstaller {
        version: version.clone(),
        display_version: display,
        urls: vec![
            format!("{maven}/{version}/{file}"),
            format!("{mirror}/{version}/{file}"),
        ],
    })
}

fn latest_neoforge_version(versions: &[String], prefix: &str) -> Option<String> {
    let matching: Vec<&String> = versions
        .iter()
        .filter(|candidate| neoforge_version_matches(candidate, prefix))
        .collect();

    let stable = matching
        .iter()
        .filter(|candidate| !candidate.contains("alpha") && !candidate.contains("beta"))
        .max_by(|a, b| compare_versions(a, b))
        .map(|candidate| (*candidate).clone());

    stable.or_else(|| {
        matching
            .iter()
            .max_by(|a, b| compare_versions(a, b))
            .map(|candidate| (*candidate).clone())
    })
}

fn fetch_neoforge_versions(
    maven: &str,
    mirror: &str,
    origins: &Origins,
) -> Result<Vec<String>, String> {
    let apis = [api_url(maven), api_url(mirror)];

    for api in apis {
        if let Ok(json) = download::get_json(&api)
            && let Some(versions) = json["versions"].as_array()
        {
            let versions: Vec<String> = versions
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect();
            if !versions.is_empty() {
                return Ok(versions);
            }
        }
    }

    let mut last_error = String::new();
    for base in [maven, mirror] {
        match fetch_versions(&format!("{base}/maven-metadata.xml"), origins) {
            Ok(versions) => return Ok(versions),
            Err(error) => last_error = error,
        }
    }

    Err(last_error)
}

fn api_url(maven: &str) -> String {
    match maven.split_once("/releases/") {
        Some((host, path)) => format!("{host}/api/maven/versions/releases/{path}"),
        None => format!("{maven}/maven-metadata.xml"),
    }
}

pub fn neoforge_prefix(mc_version: &str) -> Result<String, String> {
    if mc_version == "1.20.1" {
        return Ok(String::from("1.20.1-"));
    }

    let parts: Vec<&str> = mc_version.split('.').collect();
    if parts.len() < 2 {
        return Err(format!(
            "unsupported Minecraft version for NeoForge: {mc_version}"
        ));
    }

    if parts[0] == "1" {
        let minor = parts[1];
        let patch = parts.get(2).copied().unwrap_or("0");
        return Ok(format!("{minor}.{patch}"));
    }

    Ok(format!("{}.{}", parts[0], parts[1]))
}

fn forge_artifact_matches(candidate: &str, mc_version: &str, loader_version: &str) -> bool {
    let prefix = format!("{mc_version}-");
    if !candidate.starts_with(&prefix) {
        return false;
    }

    let rest = &candidate[prefix.len()..];
    rest == loader_version
        || rest.starts_with(&format!("{loader_version}-"))
        || rest
            .strip_suffix(&format!("-{mc_version}"))
            .map(|value| value == loader_version)
            .unwrap_or(false)
}

fn forge_artifact(mc_version: &str, version: &str) -> String {
    let minor = mc_version
        .strip_prefix("1.")
        .and_then(|rest| rest.split('.').next())
        .and_then(|minor| minor.parse::<u32>().ok());

    match minor {
        Some(minor) if minor < 9 => format!("{mc_version}-{version}-{mc_version}"),
        _ => format!("{mc_version}-{version}"),
    }
}

fn normalize_forge_version(version: &str) -> String {
    version.trim().trim_start_matches("forge-").to_string()
}

fn normalize_neoforge_version(version: &str, prefix: &str) -> String {
    let version = version
        .trim()
        .trim_start_matches("neoforge-")
        .trim_start_matches("forge-");

    if version.starts_with(prefix) {
        version.to_string()
    } else if prefix.ends_with('-') {
        format!("{prefix}{version}")
    } else {
        format!("{prefix}.{version}")
    }
}

fn neoforge_version_matches(version: &str, prefix: &str) -> bool {
    version
        .strip_prefix(prefix)
        .map(|rest| {
            if prefix.ends_with('-') {
                rest.starts_with(|c: char| c.is_ascii_digit())
            } else {
                rest.starts_with('.')
            }
        })
        .unwrap_or(false)
}

fn fetch_versions(url: &str, origins: &Origins) -> Result<Vec<String>, String> {
    let xml = download::get_text(url, origins)?;
    let mut versions = Vec::new();

    for chunk in xml.split("<version>").skip(1) {
        let version = chunk.split("</version>").next().unwrap_or("").trim();
        if !version.is_empty() {
            versions.push(version.to_string());
        }
    }

    if versions.is_empty() {
        return Err(format!("no versions in {url}"));
    }

    Ok(versions)
}

fn compare_versions(left: &str, right: &str) -> std::cmp::Ordering {
    version_key(left).cmp(&version_key(right))
}

fn version_key(version: &str) -> Vec<u32> {
    version
        .split(|c: char| !c.is_ascii_digit())
        .filter(|part| !part.is_empty())
        .map(|part| part.parse().unwrap_or(0))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefixes_follow_neoforge_layout() {
        assert_eq!(neoforge_prefix("1.20.1").unwrap(), "1.20.1-");
        assert_eq!(neoforge_prefix("1.20.2").unwrap(), "20.2");
        assert_eq!(neoforge_prefix("1.21").unwrap(), "21.0");
        assert_eq!(neoforge_prefix("1.21.1").unwrap(), "21.1");
    }

    #[test]
    fn latest_neoforge_prefers_stable_release() {
        let versions = vec![
            String::from("21.1.10"),
            String::from("21.1.11-beta"),
            String::from("21.2.1"),
        ];
        assert_eq!(
            latest_neoforge_version(&versions, "21.1").as_deref(),
            Some("21.1.10")
        );
    }

    #[test]
    fn loader_versions_sort_numerically() {
        assert!(compare_versions("21.1.10", "21.1.9") == std::cmp::Ordering::Greater);
        assert!(
            compare_versions("1.20.1-47.1.106", "1.20.1-47.1.99") == std::cmp::Ordering::Greater
        );
    }

    #[test]
    fn forge_artifact_handles_legacy_layout() {
        assert_eq!(
            forge_artifact("1.7.10", "10.13.4.1614"),
            "1.7.10-10.13.4.1614-1.7.10"
        );
        assert_eq!(forge_artifact("1.20.1", "47.2.0"), "1.20.1-47.2.0");
    }
}
