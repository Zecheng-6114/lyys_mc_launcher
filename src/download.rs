
use std::fs;
use std::io::Read;
use std::path::Path;

use serde_json::Value;

use crate::launch::needed_libraries;

const MOJANG_MANIFEST: &str = "https://piston-meta.mojang.com/mc/game/version_manifest_v2.json";

fn manifest_url(mirror: Option<&str>) -> String {
    match mirror {
        Some(base) => format!("{base}/version_manifest_v2.json"),
        None => String::from(MOJANG_MANIFEST),
    }
}

pub fn fetch(url: &str, dest: &Path, size: Option<u64>) -> Result<(), String> {
    if let Some(dir) = dest.parent() {
        if !dir.as_os_str().is_empty() {
            fs::create_dir_all(dir).map_err(|e| format!("failed to create dir {}: {e}", dir.display()))?;
        }
    }

    if dest.exists() {
        let same = match (fs::metadata(dest), size) {
            (Ok(meta), Some(want)) => meta.len() == want,
            (Ok(_), None) => true,
            _ => false,
        };
        if same {
            return Ok(());
        }
    }

    println!("downloading {url}");
    let resp = ureq::get(url)
        .call()
        .map_err(|e| format!("request failed {url}: {e}"))?;

    let mut bytes = Vec::new();
    resp.into_reader()
        .read_to_end(&mut bytes)
        .map_err(|e| format!("read failed {url}: {e}"))?;

    if let Some(want) = size {
        if bytes.len() as u64 != want {
            return Err(format!(
                "size mismatch {url}\n expected {want}\n got {}",
                bytes.len()
            ));
        }
    }

    fs::write(dest, bytes).map_err(|e| format!("failed to write {}: {e}", dest.display()))?;
    Ok(())
}

pub fn ensure_version_json(mc_dir: &str, id: &str, mirror: Option<&str>) -> Result<Value, String> {
    let path = Path::new(mc_dir)
        .join("versions")
        .join(id)
        .join(format!("{id}.json"));

    if !path.exists() {
        let url = manifest_url(mirror);
        println!("no local {id}.json, fetching manifest {url}");
        let manifest = get_json(&url)?;

        let entry = manifest["versions"]
            .as_array()
            .unwrap()
            .iter()
            .find(|v| v["id"] == id)
            .ok_or_else(|| format!("version {id} not found in manifest"))?;

        let url = entry["url"].as_str().unwrap();
        fetch(url, &path, None)?;
    }

    let text = fs::read_to_string(&path).map_err(|e| format!("failed to read {}: {e}", path.display()))?;
    serde_json::from_str(&text).map_err(|e| format!("failed to parse {}: {e}", path.display()))
}

pub fn ensure_version_files(mc_dir: &str, version: &Value) -> Result<(), String> {
    let id = version["id"].as_str().unwrap();
    let client = &version["downloads"]["client"];

    let jar = Path::new(mc_dir)
        .join("versions")
        .join(id)
        .join(format!("{id}.jar"));
    fetch(
        client["url"].as_str().unwrap(),
        &jar,
        client["size"].as_u64(),
    )?;

    for lib in needed_libraries(version) {
        let art = &lib["downloads"]["artifact"];
        let (Some(url), Some(path)) = (art["url"].as_str(), art["path"].as_str()) else {
            continue;
        };
        let dest = Path::new(mc_dir).join("libraries").join(path);
        fetch(url, &dest, art["size"].as_u64())?;
    }

    Ok(())
}

fn get_json(url: &str) -> Result<Value, String> {
    let resp = ureq::get(url)
        .call()
        .map_err(|e| format!("request failed {url}: {e}"))?;

    let mut text = String::new();
    resp.into_reader()
        .read_to_string(&mut text)
        .map_err(|e| format!("read failed {url}: {e}"))?;

    serde_json::from_str(&text).map_err(|e| format!("failed to parse {url}: {e}"))
}

pub fn load_version_json(mc_dir: &str, id: &str) -> Result<Value, String> {
    let path = Path::new(mc_dir)
        .join("versions")
        .join(id)
        .join(format!("{id}.json"));
    let text = fs::read_to_string(&path).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => format!(
            "version {id} is not installed; run: lyys_mc_launcher download --dir <game_dir> --version {id}"
        ),
        _ => format!("failed to read {}: {e}", path.display()),
    })?;
    serde_json::from_str(&text).map_err(|e| format!("failed to parse {}: {e}", path.display()))
}

pub fn download_instance(
    mc_dir: &str,
    instance: &str,
    source: &str,
    mirror: Option<&str>,
) -> Result<(), String> {
    let mut version = ensure_version_json(mc_dir, source, mirror)?;

    version["id"] = Value::String(instance.to_string());

    let dir = Path::new(mc_dir).join("versions").join(instance);
    fs::create_dir_all(&dir).map_err(|e| format!("failed to create dir {}: {e}", dir.display()))?;
    let text = serde_json::to_string_pretty(&version).map_err(|e| e.to_string())?;
    fs::write(dir.join(format!("{instance}.json")), text)
        .map_err(|e| format!("failed to write version.json: {e}"))?;

    ensure_version_files(mc_dir, &version)?;
    Ok(())
}

pub fn ensure_assets(mc_dir: &str, version: &Value) -> Result<(), String> {
    let idx = &version["assetIndex"];
    let id = idx["id"].as_str().unwrap();
    let index_path = Path::new(mc_dir)
        .join("assets")
        .join("indexes")
        .join(format!("{id}.json"));

    if !index_path.exists() {
        fetch(idx["url"].as_str().unwrap(), &index_path, idx["size"].as_u64())?;
    }

    let text = fs::read_to_string(&index_path)
        .map_err(|e| format!("failed to read asset index {}: {e}", index_path.display()))?;
    let index: Value =
        serde_json::from_str(&text).map_err(|e| format!("failed to parse asset index: {e}"))?;

    let objects = index["objects"]
        .as_object()
        .ok_or_else(|| String::from("asset index has no objects"))?;

    for obj in objects.values() {
        let hash = obj["hash"].as_str().unwrap();
        let size = obj["size"].as_u64().unwrap();
        let prefix = &hash[..2];
        let dest = Path::new(mc_dir)
            .join("assets")
            .join("objects")
            .join(prefix)
            .join(hash);
        let url = format!("https://resources.download.minecraft.net/{prefix}/{hash}");
        fetch(&url, &dest, Some(size))?;
    }

    Ok(())
}