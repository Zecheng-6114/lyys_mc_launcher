use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Read;
use std::path::Path;
use std::process::Command;

use serde_json::Value;

pub struct LaunchOptions {
    pub memory_mb: u32,
    pub username: String,
    pub uuid: String,
    pub access_token: String,
    pub game_dir: String,
}

pub fn build_args(
    mc_dir: &str,
    version: &Value,
    jar_id: &str,
    opts: &LaunchOptions,
) -> Vec<String> {
    let id = version["id"].as_str().unwrap_or("unknown");
    let asset_index = version["assetIndex"]["id"]
        .as_str()
        .or_else(|| version["assets"].as_str())
        .unwrap_or("legacy");
    let version_type = version["type"].as_str().unwrap_or("release");
    let natives = format!("{mc_dir}/natives");
    let classpath = classpath_of(mc_dir, version, jar_id);
    let features = default_features();
    let game_assets = game_assets_dir(mc_dir, asset_index);

    let vars: Vec<(&str, String)> = vec![
        ("${auth_player_name}", opts.username.clone()),
        ("${version_name}.jar", format!("{jar_id}.jar")),
        ("${version_name}", id.into()),
        ("${game_directory}", opts.game_dir.clone()),
        ("${assets_root}", format!("{mc_dir}/assets")),
        ("${game_assets}", game_assets),
        ("${assets_index_name}", asset_index.into()),
        ("${auth_uuid}", opts.uuid.clone()),
        ("${auth_access_token}", opts.access_token.clone()),
        ("${clientid}", "0".into()),
        ("${auth_xuid}", "0".into()),
        ("${version_type}", version_type.into()),
        ("${natives_directory}", natives.clone()),
        ("${launcher_name}", env!("CARGO_PKG_NAME").into()),
        ("${launcher_version}", env!("CARGO_PKG_VERSION").into()),
        ("${classpath}", classpath.clone()),
        ("${classpath_separator}", path_separator().to_string()),
        ("${library_directory}", format!("{mc_dir}/libraries")),
        ("${user_type}", "legacy".into()),
        ("${user_properties}", "{}".into()),
        ("${resolution_width}", "854".into()),
        ("${resolution_height}", "480".into()),
    ];

    let mut args = vec![format!("-Xmx{}m", opts.memory_mb)];

    if version["arguments"]["jvm"].is_array() {
        push_items(&mut args, &version["arguments"]["jvm"], &vars, &features);
    } else {
        args.push(format!("-Djava.library.path={natives}"));
        args.push(String::from("-cp"));
        args.push(classpath);
    }

    args.push(
        version["mainClass"]
            .as_str()
            .unwrap_or("net.minecraft.client.main.Main")
            .into(),
    );

    if version["arguments"]["game"].is_array() {
        push_items(&mut args, &version["arguments"]["game"], &vars, &features);
    } else if let Some(line) = version["minecraftArguments"].as_str() {
        for token in tokenize(line) {
            args.push(fill(&token, &vars));
        }
    }

    args
}

fn push_items(
    args: &mut Vec<String>,
    items: &Value,
    vars: &[(&str, String)],
    features: &HashMap<&str, bool>,
) {
    for item in items.as_array().into_iter().flatten() {
        if let Some(value) = item.as_str() {
            args.push(fill(value, vars));
            continue;
        }

        if !rules_allow(&item["rules"], features) {
            continue;
        }

        match &item["value"] {
            Value::String(value) => args.push(fill(value, vars)),
            Value::Array(values) => {
                for value in values.iter().filter_map(Value::as_str) {
                    args.push(fill(value, vars));
                }
            }
            _ => {}
        }
    }
}

fn game_assets_dir(mc_dir: &str, asset_index: &str) -> String {
    let index = Path::new(mc_dir)
        .join("assets")
        .join("indexes")
        .join(format!("{asset_index}.json"));
    if let Ok(text) = fs::read_to_string(index)
        && let Ok(json) = serde_json::from_str::<Value>(&text)
        && json["virtual"].as_bool() == Some(true)
    {
        return format!("{mc_dir}/assets/virtual/{asset_index}");
    }

    format!("{mc_dir}/assets")
}

fn default_features() -> HashMap<&'static str, bool> {
    HashMap::from([
        ("is_demo_user", false),
        ("has_custom_resolution", false),
        ("has_quick_plays_support", false),
        ("is_quick_play_singleplayer", false),
        ("is_quick_play_multiplayer", false),
        ("is_quick_play_realms", false),
    ])
}

fn rules_allow(rules: &Value, features: &HashMap<&str, bool>) -> bool {
    let Some(rules) = rules.as_array() else {
        return true;
    };
    if rules.is_empty() {
        return true;
    }

    let mut allowed = false;
    for rule in rules {
        if !rule_features_match(&rule["features"], features) {
            continue;
        }
        if !rule_os_match(&rule["os"]) {
            continue;
        }
        allowed = rule["action"].as_str() == Some("allow");
    }
    allowed
}

fn rule_features_match(features: &Value, actual: &HashMap<&str, bool>) -> bool {
    let Some(features) = features.as_object() else {
        return true;
    };

    features.iter().all(|(key, expected)| {
        expected.as_bool().unwrap_or(false) == actual.get(key.as_str()).copied().unwrap_or(false)
    })
}

fn rule_os_match(os: &Value) -> bool {
    let Some(os) = os.as_object() else {
        return true;
    };

    if let Some(name) = os.get("name").and_then(Value::as_str)
        && name != os_name()
    {
        return false;
    }

    if let Some(arch) = os.get("arch").and_then(Value::as_str)
        && !arch_matches(arch)
    {
        return false;
    }

    true
}

fn os_name() -> &'static str {
    if cfg!(windows) {
        "windows"
    } else if cfg!(target_os = "macos") {
        "osx"
    } else {
        "linux"
    }
}

fn arch_matches(arch: &str) -> bool {
    match arch {
        "x86" => cfg!(target_pointer_width = "32"),
        "x86_64" | "amd64" => cfg!(target_pointer_width = "64"),
        "arm64" | "aarch64" => cfg!(target_arch = "aarch64"),
        _ => true,
    }
}

pub fn needed_libraries(version: &Value) -> Vec<&Value> {
    let features = default_features();
    version["libraries"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|library| rules_allow(&library["rules"], &features))
        .collect()
}

pub struct LibraryFile {
    pub path: String,
    pub url: String,
    pub size: Option<u64>,
    pub sha1: Option<String>,
}

pub fn library_file(lib: &Value) -> Option<LibraryFile> {
    if let Some(path) = lib["downloads"]["artifact"]["path"].as_str() {
        let url = lib["downloads"]["artifact"]["url"]
            .as_str()
            .filter(|url| !url.is_empty())
            .map(str::to_string)
            .or_else(|| {
                let name = lib["name"].as_str()?;
                Some(format!("{}{path}", default_maven_base(name)))
            })?;

        return Some(LibraryFile {
            path: path.to_string(),
            url,
            size: lib["downloads"]["artifact"]["size"].as_u64(),
            sha1: lib["downloads"]["artifact"]["sha1"]
                .as_str()
                .map(str::to_string),
        });
    }

    if lib["downloads"].is_object() {
        return None;
    }

    let name = lib["name"].as_str()?;
    let path = descriptor_path(name)?;
    let base = lib["url"].as_str().unwrap_or(default_maven_base(name));
    let base = if base.ends_with('/') {
        base.to_string()
    } else {
        format!("{base}/")
    };

    Some(LibraryFile {
        path: path.clone(),
        url: format!("{base}{path}"),
        size: None,
        sha1: None,
    })
}

fn default_maven_base(name: &str) -> &'static str {
    let group = name.split(':').next().unwrap_or_default();
    match group {
        "net.minecraftforge" | "de.oceanlabs.mcp" | "net.minecraft" => {
            "https://maven.minecraftforge.net/"
        }
        "net.neoforged" => "https://maven.neoforged.net/releases/",
        _ => "https://libraries.minecraft.net/",
    }
}

pub struct NativeFile {
    pub path: String,
    pub url: String,
    pub size: Option<u64>,
    pub sha1: Option<String>,
    pub excludes: Vec<String>,
}

pub fn native_files(version: &Value) -> Vec<NativeFile> {
    let mut out = Vec::new();

    for library in needed_libraries(version) {
        let Some(classifier) = library["natives"][os_name()].as_str() else {
            continue;
        };
        if classifier.is_empty() {
            continue;
        }
        let classifier = classifier.replace(
            "${arch}",
            if cfg!(target_pointer_width = "64") {
                "64"
            } else {
                "32"
            },
        );

        let excludes = library["extract"]["exclude"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .filter(|pattern| !pattern.is_empty())
            .map(str::to_string)
            .collect::<Vec<_>>();

        let download = &library["downloads"]["classifiers"][classifier.as_str()];
        let path = match download["path"].as_str() {
            Some(path) => path.to_string(),
            None => {
                let Some(name) = library["name"].as_str() else {
                    continue;
                };
                let Some(path) = classifier_path(name, &classifier) else {
                    continue;
                };
                path
            }
        };

        let url = match download["url"].as_str() {
            Some(url) => url.to_string(),
            None => {
                let base = library["url"]
                    .as_str()
                    .unwrap_or("https://libraries.minecraft.net/");
                let base = if base.ends_with('/') {
                    base.to_string()
                } else {
                    format!("{base}/")
                };
                format!("{base}{path}")
            }
        };

        out.push(NativeFile {
            path,
            url,
            size: download["size"].as_u64(),
            sha1: download["sha1"].as_str().map(str::to_string),
            excludes,
        });
    }

    out
}

pub fn prepare_natives(mc_dir: &str, version: &Value) -> Result<(), String> {
    let natives = Path::new(mc_dir).join("natives");
    fs::create_dir_all(&natives)
        .map_err(|e| format!("failed to create {}: {e}", natives.display()))?;

    for native in native_files(version) {
        let jar = Path::new(mc_dir).join("libraries").join(&native.path);
        let file = fs::File::open(&jar)
            .map_err(|e| format!("failed to open native jar {}: {e}", jar.display()))?;
        let mut archive = zip::ZipArchive::new(file)
            .map_err(|e| format!("failed to read native jar {}: {e}", jar.display()))?;

        for index in 0..archive.len() {
            let mut entry = archive
                .by_index(index)
                .map_err(|e| format!("failed to read {}: {e}", jar.display()))?;
            if entry.is_dir() {
                continue;
            }

            let name = entry.name().to_string();
            if name.starts_with("META-INF/")
                || native
                    .excludes
                    .iter()
                    .any(|pattern| name.starts_with(pattern) || wildcard_match(pattern, &name))
            {
                continue;
            }

            let relative = match safe_relative(&name) {
                Some(relative) => relative,
                None => continue,
            };
            let dest = natives.join(relative);
            if let Some(dir) = dest.parent() {
                fs::create_dir_all(dir)
                    .map_err(|e| format!("failed to create {}: {e}", dir.display()))?;
            }

            let mut bytes = Vec::new();
            entry
                .read_to_end(&mut bytes)
                .map_err(|e| format!("failed to read {name}: {e}"))?;
            fs::write(&dest, bytes)
                .map_err(|e| format!("failed to write {}: {e}", dest.display()))?;
        }
    }

    Ok(())
}

fn safe_relative(name: &str) -> Option<String> {
    let mut out = Vec::new();
    for part in name.split(['/', '\\']) {
        match part {
            "" | "." => {}
            ".." => return None,
            _ => out.push(part),
        }
    }
    if out.is_empty() {
        None
    } else {
        Some(out.join(std::path::MAIN_SEPARATOR_STR))
    }
}

fn wildcard_match(pattern: &str, text: &str) -> bool {
    let pattern: Vec<char> = pattern.chars().collect();
    let text: Vec<char> = text.chars().collect();
    let mut dp = vec![vec![false; text.len() + 1]; pattern.len() + 1];
    dp[0][0] = true;

    for i in 1..=pattern.len() {
        if pattern[i - 1] == '*' {
            dp[i][0] = dp[i - 1][0];
        }
    }

    for i in 1..=pattern.len() {
        for j in 1..=text.len() {
            dp[i][j] = match pattern[i - 1] {
                '*' => dp[i - 1][j] || dp[i][j - 1],
                '?' => dp[i - 1][j - 1],
                c => dp[i - 1][j - 1] && c == text[j - 1],
            };
        }
    }

    dp[pattern.len()][text.len()]
}

fn descriptor_path(name: &str) -> Option<String> {
    let (coordinate, ext) = match name.split_once('@') {
        Some((coordinate, ext)) => (coordinate, ext),
        None => (name, "jar"),
    };
    let parts: Vec<&str> = coordinate.split(':').collect();
    if parts.len() < 3 {
        return None;
    }

    let group = parts[0].replace('.', "/");
    let artifact = parts[1];
    let version = parts[2];
    let classifier = parts.get(3).filter(|value| !value.is_empty());
    let file = match classifier {
        Some(classifier) => format!("{artifact}-{version}-{classifier}.{ext}"),
        None => format!("{artifact}-{version}.{ext}"),
    };

    Some(format!("{group}/{artifact}/{version}/{file}"))
}

fn classifier_path(name: &str, classifier: &str) -> Option<String> {
    let coordinate = name.split('@').next().unwrap_or(name);
    let parts: Vec<&str> = coordinate.split(':').collect();
    if parts.len() < 3 {
        return None;
    }
    descriptor_path(&format!("{}:{}", parts[..3].join(":"), classifier))
}

fn classpath_of(mc_dir: &str, version: &Value, jar_id: &str) -> String {
    let mut parts = vec![format!("{mc_dir}/versions/{jar_id}/{jar_id}.jar")];

    for lib in needed_libraries(version) {
        if let Some(file) = library_file(lib) {
            parts.push(format!("{mc_dir}/libraries/{}", file.path));
        }
    }

    let mut seen = HashSet::new();
    parts.retain(|path| seen.insert(path.clone()));

    parts.join(path_separator())
}

fn path_separator() -> &'static str {
    if cfg!(windows) { ";" } else { ":" }
}

fn fill(template: &str, vars: &[(&str, String)]) -> String {
    let mut out = template.to_string();
    for (key, value) in vars {
        out = out.replace(*key, value);
    }
    out
}

fn tokenize(input: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut escaped = false;

    for c in input.chars() {
        if escaped {
            current.push(c);
            escaped = false;
            continue;
        }

        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), '\\') => escaped = true,
            (Some(_), c) => current.push(c),
            (None, '\\') => escaped = true,
            (None, '"' | '\'') => quote = Some(c),
            (None, c) if c.is_whitespace() => {
                if !current.is_empty() {
                    out.push(std::mem::take(&mut current));
                }
            }
            (None, c) => current.push(c),
        }
    }

    if escaped {
        current.push('\\');
    }
    if !current.is_empty() {
        out.push(current);
    }

    out
}

pub fn resolve_java(explicit: Option<&str>) -> String {
    if let Some(java) = explicit {
        return java.to_string();
    }

    if let Some(home) = std::env::var_os("JAVA_HOME") {
        let candidate =
            Path::new(&home)
                .join("bin")
                .join(if cfg!(windows) { "java.exe" } else { "java" });
        if candidate.is_file() {
            return candidate.to_string_lossy().to_string();
        }
    }

    String::from("java")
}

pub fn check_java(java: &str, version: &Value) -> Result<(), String> {
    let required = version["javaVersion"]["majorVersion"].as_u64().unwrap_or(8) as u32;
    let actual = java_major(java)?;

    if actual < required {
        return Err(format!(
            "Java {required}+ is required for this instance, but {} provides Java {actual}",
            java
        ));
    }

    Ok(())
}

fn java_major(java: &str) -> Result<u32, String> {
    let output = Command::new(java)
        .arg("-version")
        .output()
        .map_err(|e| format!("failed to run {java}: {e}"))?;

    let mut text = String::from_utf8_lossy(&output.stdout).to_string();
    text.push_str(&String::from_utf8_lossy(&output.stderr));

    let start = text
        .find("version \"")
        .map(|index| index + "version \"".len())
        .ok_or_else(|| format!("failed to detect Java version from {java}"))?;
    let rest = &text[start..];
    let version = rest.split('"').next().unwrap_or("");

    if let Some(rest) = version.strip_prefix("1.") {
        return rest
            .split(['.', '_'])
            .next()
            .and_then(|value| value.parse().ok())
            .ok_or_else(|| format!("failed to parse Java version: {version}"));
    }

    version
        .split('.')
        .next()
        .and_then(|value| value.parse().ok())
        .ok_or_else(|| format!("failed to parse Java version: {version}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokenizes_quoted_arguments() {
        assert_eq!(
            tokenize("--username Steve --demo \"a b\" 'c d'"),
            vec!["--username", "Steve", "--demo", "a b", "c d"]
        );
    }

    #[test]
    fn wildcard_matching_handles_prefixes() {
        assert!(wildcard_match("META-INF/*", "META-INF/MANIFEST.MF"));
        assert!(wildcard_match("*.md", "readme.md"));
        assert!(!wildcard_match("*.md", "readme.txt"));
    }

    #[test]
    fn rules_evaluate_os_and_features() {
        let features = default_features();
        let rules = serde_json::json!([
            {"action": "allow", "os": {"name": os_name()}},
            {"action": "disallow", "os": {"name": "windows"}}
        ]);
        let expected = os_name() != "windows";
        assert_eq!(rules_allow(&rules, &features), expected);

        let demo = serde_json::json!([
            {"action": "allow", "features": {"is_demo_user": true}}
        ]);
        assert!(!rules_allow(&demo, &features));
    }
}
