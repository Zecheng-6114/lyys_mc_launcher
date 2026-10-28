use std::fs;
use std::path::Path;

use serde_json::Value;

use crate::download::{self, Origins};

const FABRIC_META: &str = "https://meta.fabricmc.net/v2";

pub fn install_fabric(
    mc_dir: &str,
    mc_version: &str,
    loader_version: Option<&str>,
    name: Option<&str>,
    origins: &Origins,
) -> Result<String, String> {
    let base = download::ensure_version_json(mc_dir, mc_version, origins)?;

    let version = match loader_version {
        Some(v) => v.to_string(),
        None => latest_stable_loader(mc_version)?,
    };

    let mut profile = download::get_json(&format!(
        "{FABRIC_META}/versions/loader/{mc_version}/{version}/profile/json"
    ))?;

    let id = match name {
        Some(n) => n.to_string(),
        None => format!("fabric-loader-{version}-{mc_version}"),
    };
    profile["id"] = Value::String(id.clone());

    let dir = Path::new(mc_dir).join("versions").join(&id);
    fs::create_dir_all(&dir).map_err(|e| format!("failed to create dir {}: {e}", dir.display()))?;
    let text = serde_json::to_string_pretty(&profile).map_err(|e| e.to_string())?;
    fs::write(dir.join(format!("{id}.json")), text)
        .map_err(|e| format!("failed to write version.json: {e}"))?;

    download::ensure_version_files(mc_dir, &base, mc_version, origins)?;
    download::ensure_libraries(mc_dir, &profile, origins)?;

    Ok(id)
}

fn latest_stable_loader(mc_version: &str) -> Result<String, String> {
    let list = download::get_json(&format!("{FABRIC_META}/versions/loader/{mc_version}"))?;

    list.as_array()
        .and_then(|items| {
            items
                .iter()
                .find(|item| item["stable"].as_bool() == Some(true))
                .and_then(|item| item["version"].as_str())
        })
        .map(|v| v.to_string())
        .ok_or_else(|| format!("no stable Fabric Loader for {mc_version}"))
}
