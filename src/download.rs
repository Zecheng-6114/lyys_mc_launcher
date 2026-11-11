use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use serde_json::Value;
use sha1::{Digest, Sha1};
use sha2::Sha512;

use crate::launch::{library_file, native_files, needed_libraries};

const MOJANG_MANIFEST: &str = "https://piston-meta.mojang.com/mc/game/version_manifest_v2.json";

fn agent() -> &'static ureq::Agent {
    static AGENT: OnceLock<ureq::Agent> = OnceLock::new();
    AGENT.get_or_init(|| {
        ureq::Agent::config_builder()
            .timeout_connect(Some(Duration::from_secs(20)))
            .timeout_global(Some(Duration::from_secs(600)))
            .build()
            .new_agent()
    })
}

#[derive(Clone, Copy)]
pub struct Origins<'a> {
    pub mojang: Option<&'a str>,
    pub maven: Option<&'a str>,
}

fn rewrite(url: &str, origins: &Origins) -> (String, bool) {
    if let Some(base) = origins.mojang {
        let base = base.trim_end_matches('/');
        let rules = [
            (
                "https://libraries.minecraft.net/",
                format!("{base}/libraries/"),
            ),
            (
                "https://resources.download.minecraft.net/",
                format!("{base}/assets/"),
            ),
            ("https://piston-data.mojang.com/", format!("{base}/")),
            ("https://piston-meta.mojang.com/", format!("{base}/")),
            ("https://launcher.mojang.com/", format!("{base}/")),
            ("https://launchermeta.mojang.com/", format!("{base}/")),
            ("https://meta.fabricmc.net/", format!("{base}/fabric-meta/")),
        ];

        for (from, to) in &rules {
            if let Some(rest) = url.strip_prefix(from) {
                return (format!("{to}{rest}"), true);
            }
        }
    }

    if let Some(base) = origins.maven
        && let Some(rest) = url.strip_prefix("https://")
        && let Some((host, path)) = rest.split_once('/')
        && is_maven_host(host)
    {
        let base = base.trim_end_matches('/');
        let path = match host {
            "files.minecraftforge.net" => path.strip_prefix("maven/").unwrap_or(path),
            "maven.neoforged.net" => path.strip_prefix("releases/").unwrap_or(path),
            "repo1.maven.org" | "repo.maven.apache.org" => {
                path.strip_prefix("maven2/").unwrap_or(path)
            }
            _ => path,
        };
        return (format!("{base}/{path}"), true);
    }

    (String::from(url), false)
}

fn candidates(url: &str, origins: &Origins) -> Vec<String> {
    let (target, fallback) = rewrite(url, origins);
    let mut out = vec![target.clone()];

    if fallback && target != url {
        out.push(url.to_string());
    }

    for candidate in [url, target.as_str()] {
        if let Some(mirror) = neoforge_mirror(candidate)
            && !out.contains(&mirror)
        {
            out.push(mirror);
        }
    }

    out
}

fn neoforge_mirror(url: &str) -> Option<String> {
    url.strip_prefix("https://maven.neoforged.net/releases/")
        .map(|rest| format!("https://neoforged.forgecdn.net/releases/{rest}"))
}

fn is_maven_host(host: &str) -> bool {
    matches!(
        host,
        "maven.minecraftforge.net"
            | "files.minecraftforge.net"
            | "maven.neoforged.net"
            | "libraries.minecraft.net"
            | "repo1.maven.org"
            | "repo.maven.apache.org"
    )
}

pub struct FileTask {
    pub urls: Vec<String>,
    pub dest: PathBuf,
    pub size: Option<u64>,
    pub sha1: Option<String>,
    pub sha512: Option<String>,
}

struct Progress {
    files_done: AtomicUsize,
    bytes_done: AtomicU64,
}

pub fn fetch(url: &str, dest: &Path, size: Option<u64>, origins: &Origins) -> Result<(), String> {
    transfer(
        &[url.to_string()],
        dest,
        size,
        None,
        None,
        origins,
        false,
        None,
        false,
    )
}

pub fn fetch_checked(
    url: &str,
    dest: &Path,
    size: Option<u64>,
    sha1: Option<&str>,
    origins: &Origins,
) -> Result<(), String> {
    transfer(
        &[url.to_string()],
        dest,
        size,
        sha1,
        None,
        origins,
        false,
        None,
        true,
    )
}

pub fn download_all(mut tasks: Vec<FileTask>, origins: &Origins) -> Result<(), String> {
    let mut seen = std::collections::HashSet::new();
    tasks.retain(|task| seen.insert(task.dest.clone()));

    let total = tasks.len();
    if total == 0 {
        return Ok(());
    }

    let progress = Progress {
        files_done: AtomicUsize::new(0),
        bytes_done: AtomicU64::new(0),
    };

    let threads = 8.min(total);
    println!("Retrieving packages...");

    let queue: Mutex<Vec<FileTask>> = Mutex::new(tasks);
    let failure: Mutex<Option<String>> = Mutex::new(None);

    std::thread::scope(|scope| {
        let mut workers = Vec::new();
        for _ in 0..threads {
            workers.push(scope.spawn(|| {
                loop {
                    let task = queue.lock().unwrap().pop();
                    let Some(task) = task else {
                        break;
                    };

                    let name = task
                        .dest
                        .file_name()
                        .map(|name| name.to_string_lossy().to_string())
                        .unwrap_or_else(|| task.dest.to_string_lossy().to_string());
                    println!("downloading {name}");

                    if let Err(error) = transfer(
                        &task.urls,
                        &task.dest,
                        task.size,
                        task.sha1.as_deref(),
                        task.sha512.as_deref(),
                        origins,
                        true,
                        Some(&progress),
                        false,
                    ) {
                        let mut slot = failure.lock().unwrap();
                        if slot.is_none() {
                            *slot = Some(error);
                        }
                        continue;
                    }

                    progress.files_done.fetch_add(1, Ordering::SeqCst);
                }
            }));
        }

        for worker in workers {
            let _ = worker.join();
        }
    });

    let done = progress.files_done.load(Ordering::Relaxed);
    println!(
        "Downloaded {done}/{total} file(s), {}",
        human_size(progress.bytes_done.load(Ordering::Relaxed) as f64)
    );

    match failure.into_inner().unwrap() {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

#[allow(clippy::too_many_arguments)]
fn transfer(
    urls: &[String],
    dest: &Path,
    size: Option<u64>,
    sha1: Option<&str>,
    sha512: Option<&str>,
    origins: &Origins,
    quiet: bool,
    progress: Option<&Progress>,
    verify_existing: bool,
) -> Result<(), String> {
    if let Some(dir) = dest.parent()
        && !dir.as_os_str().is_empty()
    {
        fs::create_dir_all(dir)
            .map_err(|e| format!("failed to create dir {}: {e}", dir.display()))?;
    }

    if dest.exists() {
        let valid = match (fs::metadata(dest), size) {
            (Ok(meta), Some(want)) if meta.len() != want => false,
            (Ok(_), _) if verify_existing && sha512.is_some() => file_sha512(dest)
                .map(|actual| actual == sha512.unwrap().to_ascii_lowercase())
                .unwrap_or(false),
            (Ok(_), _) if verify_existing && sha1.is_some() => file_sha1(dest)
                .map(|actual| actual == sha1.unwrap().to_ascii_lowercase())
                .unwrap_or(false),
            (Ok(_), _) => true,
            _ => false,
        };

        if valid {
            if let Some(progress) = progress
                && let Some(size) = size
            {
                progress.bytes_done.fetch_add(size, Ordering::Relaxed);
            }
            return Ok(());
        }

        let _ = fs::remove_file(dest);
    }

    let mut last_error = String::new();
    let mut seen = std::collections::HashSet::new();

    for url in urls {
        for candidate in candidates(url, origins) {
            if !seen.insert(candidate.clone()) {
                continue;
            }
            if !quiet {
                println!("downloading {candidate}");
            }

            for attempt in 0..3 {
                match download_attempt(&candidate, dest, size, sha1, sha512, progress) {
                    Ok(()) => return Ok(()),
                    Err(error) => {
                        last_error = error;
                        if http_error_is_client(&last_error) {
                            break;
                        }
                        if attempt < 2 {
                            std::thread::sleep(Duration::from_millis(300 * (attempt + 1) as u64));
                        }
                    }
                }
            }
        }
    }

    let _ = fs::remove_file(temp_path(dest));
    if last_error.is_empty() {
        Err(format!("no download URL for {}", dest.display()))
    } else {
        Err(last_error)
    }
}

fn download_attempt(
    url: &str,
    dest: &Path,
    size: Option<u64>,
    sha1: Option<&str>,
    sha512: Option<&str>,
    progress: Option<&Progress>,
) -> Result<(), String> {
    let resp = agent()
        .get(url)
        .call()
        .map_err(|e| format!("request failed {url}: {e}"))?;

    let temp = temp_path(dest);
    let mut file =
        fs::File::create(&temp).map_err(|e| format!("failed to create {}: {e}", temp.display()))?;
    let mut reader = resp.into_body().into_reader();
    let mut buf = [0u8; 64 * 1024];
    let mut downloaded = 0u64;
    let mut sha1_hasher = sha1.map(|_| Sha1::new());
    let mut sha512_hasher = sha512.map(|_| Sha512::new());

    loop {
        let read = reader
            .read(&mut buf)
            .map_err(|e| format!("read failed {url}: {e}"))?;
        if read == 0 {
            break;
        }

        file.write_all(&buf[..read])
            .map_err(|e| format!("failed to write {}: {e}", temp.display()))?;
        if let Some(hasher) = sha1_hasher.as_mut() {
            hasher.update(&buf[..read]);
        }
        if let Some(hasher) = sha512_hasher.as_mut() {
            hasher.update(&buf[..read]);
        }
        downloaded += read as u64;

        if let Some(progress) = progress {
            progress
                .bytes_done
                .fetch_add(read as u64, Ordering::Relaxed);
        }
    }

    file.flush()
        .map_err(|e| format!("failed to flush {}: {e}", temp.display()))?;
    drop(file);

    if let Some(want) = size
        && downloaded != want
    {
        let _ = fs::remove_file(&temp);
        return Err(format!(
            "size mismatch {url}\n expected {want}\n got {downloaded}"
        ));
    }

    if let Some(hasher) = sha512_hasher {
        let actual = hex(&hasher.finalize());
        if actual != sha512.unwrap_or_default().to_ascii_lowercase() {
            let _ = fs::remove_file(&temp);
            return Err(format!(
                "sha512 mismatch {url}\n expected {}\n got {actual}",
                sha512.unwrap_or_default()
            ));
        }
    }

    if let Some(hasher) = sha1_hasher {
        let actual = hex(&hasher.finalize());
        if actual != sha1.unwrap_or_default().to_ascii_lowercase() {
            let _ = fs::remove_file(&temp);
            return Err(format!(
                "sha1 mismatch {url}\n expected {}\n got {actual}",
                sha1.unwrap_or_default()
            ));
        }
    }

    if dest.exists() {
        fs::remove_file(dest).map_err(|e| format!("failed to replace {}: {e}", dest.display()))?;
    }
    fs::rename(&temp, dest).map_err(|e| {
        format!(
            "failed to move {} to {}: {e}",
            temp.display(),
            dest.display()
        )
    })?;
    Ok(())
}

fn temp_path(dest: &Path) -> PathBuf {
    let name = dest
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| String::from("download"));
    dest.with_file_name(format!("{name}.part"))
}

fn http_error_is_client(error: &str) -> bool {
    error.contains("http status: 4")
}

pub fn get_text(url: &str, origins: &Origins) -> Result<String, String> {
    let mut last_error = String::new();
    for candidate in candidates(url, origins) {
        for attempt in 0..3 {
            match read_text(&candidate) {
                Ok(text) => return Ok(text),
                Err(error) => {
                    last_error = error;
                    if http_error_is_client(&last_error) {
                        break;
                    }
                    if attempt < 2 {
                        std::thread::sleep(Duration::from_millis(300 * (attempt + 1) as u64));
                    }
                }
            }
        }
    }

    Err(last_error)
}

fn read_text(url: &str) -> Result<String, String> {
    let resp = agent()
        .get(url)
        .call()
        .map_err(|e| format!("request failed {url}: {e}"))?;

    let mut text = String::new();
    resp.into_body()
        .into_reader()
        .read_to_string(&mut text)
        .map_err(|e| format!("read failed {url}: {e}"))?;

    Ok(text)
}

pub fn get_json(url: &str) -> Result<Value, String> {
    let text = read_text(url)?;
    serde_json::from_str(&text).map_err(|e| format!("failed to parse {url}: {e}"))
}

pub fn get_json_with_headers(url: &str, headers: &[(&str, &str)]) -> Result<Value, String> {
    let mut request = agent().get(url);
    for (name, value) in headers {
        request = request.header(*name, *value);
    }

    let resp = request
        .call()
        .map_err(|e| format!("request failed {url}: {e}"))?;
    let mut text = String::new();
    resp.into_body()
        .into_reader()
        .read_to_string(&mut text)
        .map_err(|e| format!("read failed {url}: {e}"))?;
    serde_json::from_str(&text).map_err(|e| format!("failed to parse {url}: {e}"))
}

pub fn is_client_error(error: &str) -> bool {
    http_error_is_client(error)
}

pub fn fetch_json(url: &str, origins: &Origins) -> Result<Value, String> {
    let text = get_text(url, origins)?;
    serde_json::from_str(&text).map_err(|e| format!("failed to parse {url}: {e}"))
}

pub fn ensure_version_json(mc_dir: &str, id: &str, origins: &Origins) -> Result<Value, String> {
    let path = Path::new(mc_dir)
        .join("versions")
        .join(id)
        .join(format!("{id}.json"));

    if !path.exists() {
        println!("no local {id}.json, fetching manifest");

        let manifest = fetch_json(MOJANG_MANIFEST, origins)?;
        let entry = manifest["versions"]
            .as_array()
            .ok_or_else(|| String::from("manifest has no versions"))?
            .iter()
            .find(|v| v["id"] == id)
            .ok_or_else(|| format!("version {id} not found in manifest"))?;

        let url = entry["url"]
            .as_str()
            .ok_or_else(|| String::from("manifest entry has no url"))?;
        fetch(url, &path, None, origins)?;
    }

    let text =
        fs::read_to_string(&path).map_err(|e| format!("failed to read {}: {e}", path.display()))?;
    serde_json::from_str(&text).map_err(|e| format!("failed to parse {}: {e}", path.display()))
}

pub fn fetch_version_json_cached(
    mc_dir: &str,
    id: &str,
    origins: &Origins,
) -> Result<Value, String> {
    let primary = Path::new(mc_dir)
        .join("versions")
        .join(id)
        .join(format!("{id}.json"));
    let cached = Path::new(mc_dir)
        .join(".cache")
        .join("versions")
        .join(id)
        .join(format!("{id}.json"));

    for path in [&primary, &cached] {
        if path.is_file() {
            let text = fs::read_to_string(path)
                .map_err(|e| format!("failed to read {}: {e}", path.display()))?;
            let json: Value = serde_json::from_str(&text)
                .map_err(|e| format!("failed to parse {}: {e}", path.display()))?;
            return Ok(json);
        }
    }

    println!("no local {id}.json, fetching manifest");

    let manifest = fetch_json(MOJANG_MANIFEST, origins)?;
    let entry = manifest["versions"]
        .as_array()
        .ok_or_else(|| String::from("manifest has no versions"))?
        .iter()
        .find(|v| v["id"] == id)
        .ok_or_else(|| format!("version {id} not found in manifest"))?;

    let url = entry["url"]
        .as_str()
        .ok_or_else(|| String::from("manifest entry has no url"))?;
    if let Some(dir) = cached.parent() {
        fs::create_dir_all(dir)
            .map_err(|e| format!("failed to create dir {}: {e}", dir.display()))?;
    }
    fetch(url, &cached, None, origins)?;

    let text = fs::read_to_string(&cached)
        .map_err(|e| format!("failed to read {}: {e}", cached.display()))?;
    let json: Value = serde_json::from_str(&text)
        .map_err(|e| format!("failed to parse {}: {e}", cached.display()))
        .inspect_err(|_| {
            let _ = fs::remove_file(&cached);
        })?;
    Ok(json)
}

pub fn ensure_version_files(
    mc_dir: &str,
    version: &Value,
    jar_id: &str,
    origins: &Origins,
) -> Result<(), String> {
    let client = &version["downloads"]["client"];

    let mut tasks = vec![FileTask {
        urls: vec![
            client["url"]
                .as_str()
                .ok_or_else(|| String::from("version has no client url"))?
                .to_string(),
        ],
        dest: Path::new(mc_dir)
            .join("versions")
            .join(jar_id)
            .join(format!("{jar_id}.jar")),
        size: client["size"].as_u64(),
        sha1: client["sha1"].as_str().map(str::to_string),
        sha512: None,
    }];

    for lib in needed_libraries(version) {
        let Some(file) = library_file(lib) else {
            continue;
        };
        tasks.push(FileTask {
            urls: vec![file.url],
            dest: Path::new(mc_dir).join("libraries").join(&file.path),
            size: file.size,
            sha1: file.sha1,
            sha512: None,
        });
    }

    for native in native_files(version) {
        tasks.push(FileTask {
            urls: vec![native.url],
            dest: Path::new(mc_dir).join("libraries").join(&native.path),
            size: native.size,
            sha1: native.sha1,
            sha512: None,
        });
    }

    download_all(tasks, origins)
}

pub fn ensure_libraries(mc_dir: &str, version: &Value, origins: &Origins) -> Result<(), String> {
    let mut tasks = Vec::new();

    for lib in needed_libraries(version) {
        let Some(file) = library_file(lib) else {
            continue;
        };
        tasks.push(FileTask {
            urls: vec![file.url],
            dest: Path::new(mc_dir).join("libraries").join(&file.path),
            size: file.size,
            sha1: file.sha1,
            sha512: None,
        });
    }

    for native in native_files(version) {
        tasks.push(FileTask {
            urls: vec![native.url],
            dest: Path::new(mc_dir).join("libraries").join(&native.path),
            size: native.size,
            sha1: native.sha1,
            sha512: None,
        });
    }

    download_all(tasks, origins)
}

pub fn download_instance(
    mc_dir: &str,
    instance: &str,
    source: &str,
    origins: &Origins,
) -> Result<(), String> {
    let mut version = ensure_version_json(mc_dir, source, origins)?;

    version["id"] = Value::String(instance.to_string());

    let dir = Path::new(mc_dir).join("versions").join(instance);
    fs::create_dir_all(&dir).map_err(|e| format!("failed to create dir {}: {e}", dir.display()))?;
    let text = serde_json::to_string_pretty(&version).map_err(|e| e.to_string())?;
    fs::write(dir.join(format!("{instance}.json")), text)
        .map_err(|e| format!("failed to write version.json: {e}"))?;

    ensure_version_files(mc_dir, &version, instance, origins)?;
    Ok(())
}

pub fn ensure_assets(mc_dir: &str, version: &Value, origins: &Origins) -> Result<(), String> {
    let idx = &version["assetIndex"];
    let id = idx["id"]
        .as_str()
        .ok_or_else(|| String::from("version has no asset index id"))?;
    let index_path = Path::new(mc_dir)
        .join("assets")
        .join("indexes")
        .join(format!("{id}.json"));

    if !index_path.exists() {
        let url = idx["url"]
            .as_str()
            .ok_or_else(|| String::from("version has no asset index url"))?;
        fetch(url, &index_path, idx["size"].as_u64(), origins)?;
    }

    let text = fs::read_to_string(&index_path)
        .map_err(|e| format!("failed to read asset index {}: {e}", index_path.display()))?;
    let index: Value =
        serde_json::from_str(&text).map_err(|e| format!("failed to parse asset index: {e}"))?;

    let objects = index["objects"]
        .as_object()
        .ok_or_else(|| String::from("asset index has no objects"))?;

    let mut tasks = Vec::new();

    for obj in objects.values() {
        let hash = obj["hash"]
            .as_str()
            .ok_or_else(|| String::from("asset object has no hash"))?;
        let size = obj["size"]
            .as_u64()
            .ok_or_else(|| String::from("asset object has no size"))?;
        let prefix = &hash[..2];
        tasks.push(FileTask {
            urls: vec![format!(
                "https://resources.download.minecraft.net/{prefix}/{hash}"
            )],
            dest: Path::new(mc_dir)
                .join("assets")
                .join("objects")
                .join(prefix)
                .join(hash),
            size: Some(size),
            sha1: Some(hash.to_string()),
            sha512: None,
        });
    }

    download_all(tasks, origins)?;
    materialize_assets(mc_dir, id, &index, origins)
}

fn materialize_assets(
    mc_dir: &str,
    id: &str,
    index: &Value,
    _origins: &Origins,
) -> Result<(), String> {
    let virtual_assets = index["virtual"].as_bool() == Some(true);
    let map_to_resources = index["map_to_resources"].as_bool() == Some(true);
    if !virtual_assets && !map_to_resources {
        return Ok(());
    }

    let root = if virtual_assets {
        Path::new(mc_dir).join("assets").join("virtual").join(id)
    } else {
        Path::new(mc_dir).join("resources")
    };
    fs::create_dir_all(&root).map_err(|e| format!("failed to create {}: {e}", root.display()))?;

    let Some(objects) = index["objects"].as_object() else {
        return Ok(());
    };

    for (name, obj) in objects {
        let Some(hash) = obj["hash"].as_str() else {
            continue;
        };
        let source = Path::new(mc_dir)
            .join("assets")
            .join("objects")
            .join(&hash[..2])
            .join(hash);
        let Some(relative) = safe_asset_path(name) else {
            continue;
        };
        let dest = root.join(relative);
        if dest.exists() {
            continue;
        }
        if let Some(dir) = dest.parent() {
            fs::create_dir_all(dir)
                .map_err(|e| format!("failed to create {}: {e}", dir.display()))?;
        }
        fs::copy(&source, &dest)
            .map_err(|e| format!("failed to materialize asset {}: {e}", dest.display()))?;
    }

    Ok(())
}

fn safe_asset_path(name: &str) -> Option<PathBuf> {
    let mut out = PathBuf::new();
    for part in name.split(['/', '\\']) {
        match part {
            "" | "." => {}
            ".." => return None,
            _ => out.push(part),
        }
    }
    if out.as_os_str().is_empty() {
        None
    } else {
        Some(out)
    }
}

pub fn file_sha1(path: &Path) -> Result<String, String> {
    let mut file =
        fs::File::open(path).map_err(|e| format!("failed to open {}: {e}", path.display()))?;
    let mut hasher = Sha1::new();
    let mut buf = [0u8; 64 * 1024];

    loop {
        let read = file
            .read(&mut buf)
            .map_err(|e| format!("failed to read {}: {e}", path.display()))?;
        if read == 0 {
            break;
        }
        hasher.update(&buf[..read]);
    }

    Ok(hex(&hasher.finalize()))
}

pub fn file_sha512(path: &Path) -> Result<String, String> {
    let mut file =
        fs::File::open(path).map_err(|e| format!("failed to open {}: {e}", path.display()))?;
    let mut hasher = Sha512::new();
    let mut buf = [0u8; 64 * 1024];

    loop {
        let read = file
            .read(&mut buf)
            .map_err(|e| format!("failed to read {}: {e}", path.display()))?;
        if read == 0 {
            break;
        }
        hasher.update(&buf[..read]);
    }

    Ok(hex(&hasher.finalize()))
}

pub fn encode_path(input: &str) -> String {
    encode_uri(input, true)
}

pub fn encode_component(input: &str) -> String {
    encode_uri(input, false)
}

fn encode_uri(input: &str, allow_slash: bool) -> String {
    let mut out = String::new();
    for byte in input.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            b'/' if allow_slash => out.push('/'),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

fn human_size(bytes: f64) -> String {
    const UNITS: [&str; 4] = ["B", "KiB", "MiB", "GiB"];

    let mut value = bytes.max(0.0);
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }

    if unit == 0 {
        format!("{value:.0} {}", UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rewrites_known_mojang_hosts() {
        let origins = Origins {
            mojang: Some("https://mirror.example"),
            maven: None,
        };
        let (url, fallback) = rewrite("https://piston-meta.mojang.com/a/b.json", &origins);
        assert_eq!(url, "https://mirror.example/a/b.json");
        assert!(fallback);
    }

    #[test]
    fn rewrites_only_known_maven_hosts() {
        let origins = Origins {
            mojang: None,
            maven: Some("https://maven.example"),
        };
        assert_eq!(
            rewrite("https://maven.minecraftforge.net/net/a.jar", &origins).0,
            "https://maven.example/net/a.jar"
        );
        assert_eq!(
            rewrite("https://api.modrinth.com/v2/search", &origins).0,
            "https://api.modrinth.com/v2/search"
        );
    }

    #[test]
    fn adds_neoforge_mirror_candidate() {
        let origins = Origins {
            mojang: None,
            maven: None,
        };
        let urls = candidates(
            "https://maven.neoforged.net/releases/net/neoforged/neoforge/a.jar",
            &origins,
        );
        assert!(urls.contains(&String::from(
            "https://neoforged.forgecdn.net/releases/net/neoforged/neoforge/a.jar"
        )));
    }

    #[test]
    fn path_encoding_keeps_slashes_and_escapes_unicode() {
        assert_eq!(
            encode_path("mods/中文 文件.jar"),
            "mods/%E4%B8%AD%E6%96%87%20%E6%96%87%E4%BB%B6.jar"
        );
        assert_eq!(encode_component("a b.jar"), "a%20b.jar");
    }
}
