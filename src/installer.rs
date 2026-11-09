use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

use crate::download::{self, FileTask, Origins};

pub const INSTALL_PROFILE: &str = "install_profile.json";

pub struct Installer {
    pub profile: Value,
    pub version: Value,
    pub new_format: bool,
    pub needs_java: bool,
}

pub fn inspect(installer: &Path) -> Result<Installer, String> {
    validate_zip(installer)?;

    let text = read_zip_text(installer, INSTALL_PROFILE)?
        .ok_or_else(|| String::from("installer has no install_profile.json"))?;
    let profile: Value = serde_json::from_str(&text)
        .map_err(|e| format!("failed to parse {INSTALL_PROFILE}: {e}"))?;

    let new_format = profile.get("spec").is_some()
        || profile
            .get("processors")
            .and_then(Value::as_array)
            .map(|processors| !processors.is_empty())
            .unwrap_or(false);

    let version = if new_format {
        let path = profile["json"]
            .as_str()
            .ok_or_else(|| String::from("install_profile.json has no version json"))?
            .trim_start_matches('/');
        let text =
            read_zip_text(installer, path)?.ok_or_else(|| format!("installer has no {path}"))?;
        serde_json::from_str(&text).map_err(|e| format!("failed to parse {path}: {e}"))?
    } else {
        profile["versionInfo"].clone()
    };

    if !version.is_object() {
        return Err(String::from(
            "install_profile.json has no version information",
        ));
    }

    let needs_java = profile["processors"]
        .as_array()
        .map(|processors| {
            processors
                .iter()
                .any(|processor| processor_side(processor, "client"))
        })
        .unwrap_or(false);

    Ok(Installer {
        profile,
        version,
        new_format,
        needs_java,
    })
}

pub fn profile_minecraft(profile: &Value) -> Option<&str> {
    profile["minecraft"]
        .as_str()
        .or_else(|| profile["install"]["minecraft"].as_str())
}

pub fn install_legacy(
    mc_dir: &str,
    id: &str,
    mc_version: &str,
    installer: &Path,
    installer_info: &Installer,
    origins: &Origins,
) -> Result<(), String> {
    let profile = &installer_info.profile;
    let coordinate = profile["install"]["path"]
        .as_str()
        .ok_or_else(|| String::from("install_profile.json has no install.path"))?;
    let file_path = profile["install"]["filePath"]
        .as_str()
        .ok_or_else(|| String::from("install_profile.json has no install.filePath"))?;

    let relative = descriptor_path(coordinate)?;
    let dest = Path::new(mc_dir).join("libraries").join(&relative);
    if let Some(dir) = dest.parent() {
        fs::create_dir_all(dir).map_err(|e| format!("failed to create {}: {e}", dir.display()))?;
    }

    let bytes = read_zip_bytes(installer, file_path)?
        .ok_or_else(|| format!("installer has no {file_path}"))?;
    fs::write(&dest, bytes).map_err(|e| format!("failed to write {}: {e}", dest.display()))?;

    let mut version = installer_info.version.clone();
    version["id"] = Value::String(id.to_string());
    write_version_json(mc_dir, id, &version)?;

    download::ensure_version_json(mc_dir, mc_version, origins)?;
    download::ensure_libraries(mc_dir, &version, origins)?;
    Ok(())
}

pub fn install_new(
    mc_dir: &str,
    id: &str,
    mc_version: &str,
    installer: &Path,
    installer_info: &Installer,
    java: &str,
    origins: &Origins,
) -> Result<(), String> {
    let profile = &installer_info.profile;
    let mut version = installer_info.version.clone();
    version["id"] = Value::String(id.to_string());

    let temp = temp_dir(id)?;
    let result = install_processors(
        mc_dir, mc_version, installer, profile, &version, java, origins, &temp,
    );

    let _ = fs::remove_dir_all(&temp);
    result
}

#[allow(clippy::too_many_arguments)]
fn install_processors(
    mc_dir: &str,
    mc_version: &str,
    installer: &Path,
    profile: &Value,
    version: &Value,
    java: &str,
    origins: &Origins,
    temp: &Path,
) -> Result<(), String> {
    copy_embedded_libraries(mc_dir, installer, profile)?;
    download_profile_libraries(mc_dir, profile, origins)?;
    write_version_json(mc_dir, version["id"].as_str().unwrap_or_default(), version)?;

    let vanilla_jar = Path::new(mc_dir)
        .join("versions")
        .join(mc_version)
        .join(format!("{mc_version}.jar"));
    if !vanilla_jar.is_file() {
        return Err(format!(
            "Minecraft client jar not found: {}",
            vanilla_jar.display()
        ));
    }

    let minecraft_jar = temp.join("minecraft.jar");
    fs::copy(&vanilla_jar, &minecraft_jar)
        .map_err(|e| format!("failed to copy client jar: {e}"))?;

    let mut vars = HashMap::new();
    for (key, value) in profile["data"].as_object().cloned().unwrap_or_default() {
        let client = value["client"]
            .as_str()
            .or_else(|| value.as_str())
            .ok_or_else(|| format!("invalid install data: {key}"))?;
        let parsed = if client.is_empty() {
            String::new()
        } else if let Some(descriptor) = surrounded(client, '[', ']') {
            artifact_absolute(mc_dir, descriptor)?
        } else {
            parse_literal(client, &HashMap::new(), |plain| {
                extract_installer_file(installer, plain, temp)
            })?
        };
        vars.insert(key, parsed);
    }

    vars.insert(String::from("SIDE"), String::from("client"));
    vars.insert(
        String::from("MINECRAFT_JAR"),
        absolute(&minecraft_jar)?.to_string_lossy().to_string(),
    );
    vars.insert(String::from("MINECRAFT_VERSION"), mc_version.to_string());
    vars.insert(
        String::from("ROOT"),
        absolute(Path::new(mc_dir))?.to_string_lossy().to_string(),
    );
    vars.insert(
        String::from("INSTALLER"),
        absolute(installer)?.to_string_lossy().to_string(),
    );
    vars.insert(
        String::from("LIBRARY_DIR"),
        absolute(&Path::new(mc_dir).join("libraries"))?
            .to_string_lossy()
            .to_string(),
    );

    let processors = profile["processors"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let processors: Vec<&Value> = processors
        .iter()
        .filter(|processor| processor_side(processor, "client"))
        .collect();

    for (index, processor) in processors.iter().enumerate() {
        println!(
            "running processor {}/{}: {}",
            index + 1,
            processors.len(),
            processor["jar"].as_str().unwrap_or("unknown")
        );
        run_processor(mc_dir, processor, &vars, java, origins, temp)?;
    }

    download::ensure_version_json(mc_dir, mc_version, origins)?;
    download::ensure_libraries(mc_dir, version, origins)?;
    Ok(())
}

fn run_processor(
    mc_dir: &str,
    processor: &Value,
    vars: &HashMap<String, String>,
    java: &str,
    origins: &Origins,
    temp: &Path,
) -> Result<(), String> {
    let jar_descriptor = processor["jar"]
        .as_str()
        .ok_or_else(|| String::from("processor has no jar"))?;
    let libraries = absolute(&Path::new(mc_dir).join("libraries"))?;
    let jar = libraries.join(descriptor_path(jar_descriptor)?);

    let args = expand_args(mc_dir, processor["args"].as_array(), vars)?;

    if let Some((mappings_version, output)) = download_mojmaps(&args) {
        ensure_processor_file(&jar, jar_descriptor, origins)?;
        let version_json = download::ensure_version_json(mc_dir, &mappings_version, origins)?;
        let mappings = &version_json["downloads"]["client_mappings"];
        let url = mappings["url"]
            .as_str()
            .ok_or_else(|| String::from("version has no client_mappings url"))?;
        let size = mappings["size"].as_u64();
        let sha1 = mappings["sha1"].as_str();
        download::fetch_checked(url, Path::new(&output), size, sha1, origins)?;
        return Ok(());
    }

    let outputs = parse_outputs(mc_dir, processor["outputs"].as_object(), vars)?;
    if !outputs.is_empty() && outputs.iter().all(|(path, sha1)| file_matches(path, sha1)) {
        return Ok(());
    }
    for (path, _) in &outputs {
        let path = Path::new(path);
        if path.exists() {
            let _ = fs::remove_file(path);
        }
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)
                .map_err(|e| format!("failed to create {}: {e}", dir.display()))?;
        }
    }

    ensure_processor_file(&jar, jar_descriptor, origins)?;

    let mut classpath = Vec::new();
    for descriptor in processor["classpath"]
        .as_array()
        .cloned()
        .unwrap_or_default()
    {
        let descriptor = descriptor
            .as_str()
            .ok_or_else(|| String::from("processor classpath entry is not a string"))?;
        let path = libraries.join(descriptor_path(descriptor)?);
        ensure_processor_file(&path, descriptor, origins)?;
        classpath.push(path);
    }
    classpath.push(jar.clone());

    for pair in args.windows(2) {
        if pair[0] == "--output"
            && let Some(dir) = Path::new(&pair[1]).parent()
        {
            fs::create_dir_all(dir)
                .map_err(|e| format!("failed to create {}: {e}", dir.display()))?;
        }
    }

    let main_class = jar_main_class(&jar)?;
    let mut command = Command::new(java);
    command
        .arg("-cp")
        .arg(join_paths(&classpath))
        .arg(main_class)
        .args(&args)
        .current_dir(temp);

    let output = command
        .output()
        .map_err(|e| format!("failed to run processor with {java}: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "processor exited with {}\n--- stdout ---\n{}\n--- stderr ---\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    for (path, sha1) in outputs {
        if !Path::new(&path).is_file() {
            return Err(format!("processor did not produce {path}"));
        }
        let actual = download::file_sha1(Path::new(&path))?;
        if actual != sha1 {
            let _ = fs::remove_file(&path);
            return Err(format!(
                "processor output checksum mismatch for {path}\n expected {sha1}\n got {actual}"
            ));
        }
    }

    Ok(())
}

fn ensure_processor_file(path: &Path, descriptor: &str, origins: &Origins) -> Result<(), String> {
    if path.is_file() {
        return Ok(());
    }

    let url = maven_url(descriptor)?;
    download::fetch(&url, path, None, origins)
}

fn expand_args(
    mc_dir: &str,
    args: Option<&Vec<Value>>,
    vars: &HashMap<String, String>,
) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    for arg in args.into_iter().flatten() {
        let arg = arg
            .as_str()
            .ok_or_else(|| String::from("processor argument is not a string"))?;
        if let Some(descriptor) = surrounded(arg, '[', ']') {
            out.push(artifact_absolute(mc_dir, descriptor)?);
        } else {
            out.push(parse_literal(arg, vars, |plain| Ok(plain.to_string()))?);
        }
    }
    Ok(out)
}

fn parse_outputs(
    mc_dir: &str,
    outputs: Option<&serde_json::Map<String, Value>>,
    vars: &HashMap<String, String>,
) -> Result<Vec<(String, String)>, String> {
    let mut out = Vec::new();
    for (key, value) in outputs.into_iter().flatten() {
        let value = value
            .as_str()
            .ok_or_else(|| String::from("processor output checksum is not a string"))?;
        let path = if let Some(descriptor) = surrounded(key, '[', ']') {
            artifact_absolute(mc_dir, descriptor)?
        } else {
            parse_literal(key, vars, |plain| Ok(plain.to_string()))?
        };
        let sha1 = parse_literal(value, vars, |plain| Ok(plain.to_string()))?;
        out.push((path, sha1));
    }
    Ok(out)
}

fn surrounded(value: &str, start: char, end: char) -> Option<&str> {
    if value.len() >= 2 && value.starts_with(start) && value.ends_with(end) {
        Some(&value[start.len_utf8()..value.len() - end.len_utf8()])
    } else {
        None
    }
}

fn artifact_absolute(mc_dir: &str, descriptor: &str) -> Result<String, String> {
    Ok(absolute(
        &Path::new(mc_dir)
            .join("libraries")
            .join(descriptor_path(descriptor)?),
    )?
    .to_string_lossy()
    .to_string())
}

fn processor_side(processor: &Value, side: &str) -> bool {
    match processor["sides"].as_array() {
        Some(sides) => sides.iter().any(|value| value.as_str() == Some(side)),
        None => true,
    }
}

fn download_mojmaps(args: &[String]) -> Option<(String, String)> {
    let mut task = None;
    let mut version = None;
    let mut output = None;
    let mut index = 0;

    while index < args.len() {
        let flag = args[index].strip_prefix("--")?;
        let value = args.get(index + 1)?;
        match flag {
            "task" => task = Some(value.clone()),
            "version" => version = Some(value.clone()),
            "output" => output = Some(value.clone()),
            _ => {}
        }
        index += 2;
    }

    if task.as_deref() != Some("DOWNLOAD_MOJMAPS") {
        return None;
    }

    Some((version?, output?))
}

fn download_profile_libraries(
    mc_dir: &str,
    profile: &Value,
    origins: &Origins,
) -> Result<(), String> {
    let mut tasks = Vec::new();

    for library in profile["libraries"].as_array().into_iter().flatten() {
        let Some(file) = crate::launch::library_file(library) else {
            continue;
        };
        tasks.push(FileTask {
            url: file.url,
            dest: Path::new(mc_dir).join("libraries").join(&file.path),
            size: file.size,
            sha1: file.sha1,
        });
    }

    download::download_all(tasks, origins)
}

fn copy_embedded_libraries(mc_dir: &str, installer: &Path, profile: &Value) -> Result<(), String> {
    let libraries = profile["libraries"].as_array().cloned().unwrap_or_default();
    for library in &libraries {
        let Some(relative) = library_path(library) else {
            continue;
        };
        if let Some(bytes) = read_zip_bytes(installer, &format!("maven/{relative}"))? {
            let dest = Path::new(mc_dir).join("libraries").join(&relative);
            if let Some(dir) = dest.parent() {
                fs::create_dir_all(dir)
                    .map_err(|e| format!("failed to create {}: {e}", dir.display()))?;
            }
            fs::write(&dest, bytes)
                .map_err(|e| format!("failed to write {}: {e}", dest.display()))?;
        }
    }

    if let Some(path) = profile["path"].as_str()
        && !path.is_empty()
    {
        let relative = descriptor_path(path)?;
        if let Some(bytes) = read_zip_bytes(installer, &format!("maven/{relative}"))? {
            let dest = Path::new(mc_dir).join("libraries").join(&relative);
            if let Some(dir) = dest.parent() {
                fs::create_dir_all(dir)
                    .map_err(|e| format!("failed to create {}: {e}", dir.display()))?;
            }
            fs::write(&dest, bytes)
                .map_err(|e| format!("failed to write {}: {e}", dest.display()))?;
        }
    }

    Ok(())
}

fn write_version_json(mc_dir: &str, id: &str, version: &Value) -> Result<(), String> {
    let dir = Path::new(mc_dir).join("versions").join(id);
    fs::create_dir_all(&dir).map_err(|e| format!("failed to create {}: {e}", dir.display()))?;
    let text = serde_json::to_string_pretty(version).map_err(|e| e.to_string())?;
    fs::write(dir.join(format!("{id}.json")), text)
        .map_err(|e| format!("failed to write version json: {e}"))
}

pub fn library_path(library: &Value) -> Option<String> {
    if let Some(path) = library["downloads"]["artifact"]["path"].as_str() {
        return Some(path.to_string());
    }
    library["name"]
        .as_str()
        .and_then(|name| descriptor_path(name).ok())
}

fn file_matches(path: &str, sha1: &str) -> bool {
    let path = Path::new(path);
    path.is_file()
        && download::file_sha1(path)
            .map(|actual| actual == sha1)
            .unwrap_or(false)
}

pub fn descriptor_path(descriptor: &str) -> Result<String, String> {
    let (coordinate, ext) = match descriptor.split_once('@') {
        Some((coordinate, ext)) => (coordinate, ext),
        None => (descriptor, "jar"),
    };

    let parts: Vec<&str> = coordinate.split(':').collect();
    if parts.len() < 3 {
        return Err(format!("invalid maven coordinate: {descriptor}"));
    }

    let group = parts[0].replace('.', "/");
    let artifact = parts[1];
    let version = parts[2];
    let classifier = parts.get(3).filter(|value| !value.is_empty());

    let file = match classifier {
        Some(classifier) => format!("{artifact}-{version}-{classifier}.{ext}"),
        None => format!("{artifact}-{version}.{ext}"),
    };

    Ok(format!("{group}/{artifact}/{version}/{file}"))
}

fn maven_url(descriptor: &str) -> Result<String, String> {
    let coordinate = descriptor.split('@').next().unwrap_or(descriptor);
    let parts: Vec<&str> = coordinate.split(':').collect();
    if parts.len() < 3 {
        return Err(format!("invalid maven coordinate: {descriptor}"));
    }

    let path = descriptor_path(descriptor)?;
    let base = match parts[0] {
        "net.minecraftforge" | "de.oceanlabs.mcp" | "net.minecraft" => {
            "https://maven.minecraftforge.net"
        }
        "net.neoforged" => "https://maven.neoforged.net/releases",
        "com.mojang" | "org.lwjgl" => "https://libraries.minecraft.net",
        _ => "https://repo1.maven.org/maven2",
    };

    Ok(format!("{base}/{path}"))
}

fn extract_installer_file(installer: &Path, name: &str, temp: &Path) -> Result<String, String> {
    let name = name.trim_start_matches('/');
    let bytes =
        read_zip_bytes(installer, name)?.ok_or_else(|| format!("installer has no {name}"))?;
    let dest = temp.join(name.replace(['/', '\\'], "_"));
    fs::write(&dest, bytes).map_err(|e| format!("failed to write {}: {e}", dest.display()))?;
    Ok(absolute(&dest)?.to_string_lossy().to_string())
}

fn absolute(path: &Path) -> Result<PathBuf, String> {
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }
    std::env::current_dir()
        .map(|cwd| cwd.join(path))
        .map_err(|e| format!("failed to resolve {}: {e}", path.display()))
}

fn temp_dir(id: &str) -> Result<PathBuf, String> {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default();
    let safe: String = id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let dir = std::env::temp_dir().join(format!("lyys-installer-{safe}-{stamp}"));
    fs::create_dir_all(&dir).map_err(|e| format!("failed to create {}: {e}", dir.display()))?;
    Ok(dir)
}

fn join_paths(paths: &[PathBuf]) -> String {
    let separator = if cfg!(windows) { ";" } else { ":" };
    paths
        .iter()
        .map(|path| path.to_string_lossy().to_string())
        .collect::<Vec<_>>()
        .join(separator)
}

fn jar_main_class(jar: &Path) -> Result<String, String> {
    let text = read_zip_text(jar, "META-INF/MANIFEST.MF")?
        .ok_or_else(|| format!("{} has no manifest", jar.display()))?;

    let mut lines: Vec<String> = Vec::new();
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix(' ') {
            if let Some(last) = lines.last_mut() {
                last.push_str(rest);
            }
        } else {
            lines.push(line.trim_end_matches('\r').to_string());
        }
    }

    for line in lines {
        if let Some(value) = line.strip_prefix("Main-Class:") {
            let value = value.trim();
            if !value.is_empty() {
                return Ok(value.replace('/', "."));
            }
        }
    }

    Err(format!("{} has no Main-Class", jar.display()))
}

pub fn parse_literal<F>(
    literal: &str,
    vars: &HashMap<String, String>,
    plain: F,
) -> Result<String, String>
where
    F: Fn(&str) -> Result<String, String>,
{
    if literal.len() >= 2 && literal.starts_with('{') && literal.ends_with('}') {
        let key = &literal[1..literal.len() - 1];
        return vars
            .get(key)
            .cloned()
            .ok_or_else(|| format!("missing install variable: {key}"));
    }

    if literal.len() >= 2 && literal.starts_with('\'') && literal.ends_with('\'') {
        return Ok(literal[1..literal.len() - 1].to_string());
    }

    if literal.len() >= 2 && literal.starts_with('[') && literal.ends_with(']') {
        return descriptor_path(&literal[1..literal.len() - 1]);
    }

    plain(&expand_tokens(literal, vars)?)
}

fn expand_tokens(input: &str, vars: &HashMap<String, String>) -> Result<String, String> {
    let chars: Vec<char> = input.chars().collect();
    let mut out = String::new();
    let mut index = 0;

    while index < chars.len() {
        match chars[index] {
            '\\' => {
                index += 1;
                let Some(&next) = chars.get(index) else {
                    return Err(format!("bad escape in {input}"));
                };
                out.push(next);
                index += 1;
            }
            '{' => {
                let mut key = String::new();
                index += 1;
                loop {
                    let Some(&c) = chars.get(index) else {
                        return Err(format!("unclosed variable in {input}"));
                    };
                    index += 1;
                    match c {
                        '}' => break,
                        '\\' => {
                            let Some(&next) = chars.get(index) else {
                                return Err(format!("bad escape in {input}"));
                            };
                            key.push(next);
                            index += 1;
                        }
                        _ => key.push(c),
                    }
                }
                let value = vars
                    .get(&key)
                    .cloned()
                    .ok_or_else(|| format!("missing install variable: {key}"))?;
                out.push_str(&value);
            }
            '\'' => {
                index += 1;
                loop {
                    let Some(&c) = chars.get(index) else {
                        return Err(format!("unclosed quote in {input}"));
                    };
                    index += 1;
                    match c {
                        '\'' => break,
                        '\\' => {
                            let Some(&next) = chars.get(index) else {
                                return Err(format!("bad escape in {input}"));
                            };
                            out.push(next);
                            index += 1;
                        }
                        _ => out.push(c),
                    }
                }
            }
            c => {
                out.push(c);
                index += 1;
            }
        }
    }

    Ok(out)
}

pub fn read_zip_text(path: &Path, name: &str) -> Result<Option<String>, String> {
    let Some(bytes) = read_zip_bytes(path, name)? else {
        return Ok(None);
    };
    let text = String::from_utf8(bytes)
        .map_err(|e| format!("failed to read {name} from {}: {e}", path.display()))?;
    Ok(Some(text))
}

pub fn read_zip_bytes(path: &Path, name: &str) -> Result<Option<Vec<u8>>, String> {
    let file =
        fs::File::open(path).map_err(|e| format!("failed to open {}: {e}", path.display()))?;
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|e| format!("failed to read {}: {e}", path.display()))?;

    let mut entry = match archive.by_name(name) {
        Ok(entry) => entry,
        Err(zip::result::ZipError::FileNotFound) => return Ok(None),
        Err(e) => {
            return Err(format!(
                "failed to read {name} from {}: {e}",
                path.display()
            ));
        }
    };

    let mut bytes = Vec::new();
    entry
        .read_to_end(&mut bytes)
        .map_err(|e| format!("failed to read {name}: {e}"))?;
    Ok(Some(bytes))
}

pub fn validate_zip(path: &Path) -> Result<(), String> {
    let file =
        fs::File::open(path).map_err(|e| format!("failed to open {}: {e}", path.display()))?;
    let archive = zip::ZipArchive::new(file)
        .map_err(|e| format!("invalid installer {}: {e}", path.display()))?;
    if archive.is_empty() {
        return Err(format!(
            "invalid installer {}: archive is empty",
            path.display()
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptor_path_handles_classifier_and_extension() {
        assert_eq!(
            descriptor_path("net.minecraft:client:1.20.1").unwrap(),
            "net/minecraft/client/1.20.1/client-1.20.1.jar"
        );
        assert_eq!(
            descriptor_path("de.oceanlabs.mcp:mcp_config:1.20.1:zip@zip").unwrap(),
            "de/oceanlabs/mcp/mcp_config/1.20.1/mcp_config-1.20.1-zip.zip"
        );
    }

    #[test]
    fn literal_parsing_handles_quotes_artifacts_and_tokens() {
        let mut vars = HashMap::new();
        vars.insert(String::from("SIDE"), String::from("client"));
        vars.insert(String::from("OUT"), String::from("C:/out.jar"));

        assert_eq!(
            parse_literal("'literal'", &vars, |value| Ok(value.to_string())).unwrap(),
            "literal"
        );
        assert_eq!(
            parse_literal("[a.b:c:1.0]", &vars, |value| Ok(value.to_string())).unwrap(),
            "a/b/c/1.0/c-1.0.jar"
        );
        assert_eq!(
            parse_literal("--side {SIDE} --out {OUT}", &vars, |value| Ok(
                value.to_string()
            ))
            .unwrap(),
            "--side client --out C:/out.jar"
        );
    }

    #[test]
    fn tokens_support_escapes() {
        let vars = HashMap::new();
        assert_eq!(
            parse_literal("a\\'b", &vars, |value| Ok(value.to_string())).unwrap(),
            "a'b"
        );
    }
}
