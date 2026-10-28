
use serde_json::Value;

pub struct LaunchOptions {
    pub memory_mb: u32,
    pub username: String,
    pub uuid: String,
    pub access_token: String,
    pub game_dir: String,
}

pub fn build_args(mc_dir: &str, version: &Value, opts: &LaunchOptions) -> Vec<String> {
    let id = version["id"].as_str().unwrap();

    let vars: Vec<(&str, String)> = vec![
        ("${auth_player_name}", opts.username.clone()),
        ("${version_name}", id.into()),
        ("${game_directory}", opts.game_dir.clone()),
        ("${assets_root}", format!("{mc_dir}/assets")),
        ("${assets_index_name}", version["assetIndex"]["id"].as_str().unwrap().into()),
        ("${auth_uuid}", opts.uuid.clone()),
        ("${auth_access_token}", opts.access_token.clone()),
        ("${clientid}", "0".into()),
        ("${auth_xuid}", "0".into()),
        ("${version_type}", version["type"].as_str().unwrap().into()),
        ("${natives_directory}", format!("{mc_dir}/natives")),
        ("${launcher_name}", env!("CARGO_PKG_NAME").into()),
        ("${launcher_version}", env!("CARGO_PKG_VERSION").into()),
        ("${classpath}", classpath_of(mc_dir, version, id)),
        ("${classpath_separator}", path_separator().to_string()),
        ("${library_directory}", format!("{mc_dir}/libraries")),
        ("${user_type}", "legacy".into()),
        ("${user_properties}", "{}".into()),
        ("${resolution_width}", "854".into()),
        ("${resolution_height}", "480".into()),
    ];

    let mut args = vec![format!("-Xmx{}m", opts.memory_mb)];

    push_items(&mut args, &version["arguments"]["jvm"], &vars);
    args.push(version["mainClass"].as_str().unwrap().into());
    push_items(&mut args, &version["arguments"]["game"], &vars);

    args
}

fn push_items(args: &mut Vec<String>, items: &Value, vars: &[(&str, String)]) {
    for item in items.as_array().unwrap() {
        if let Some(s) = item.as_str() {
            args.push(fill(s, vars));
        }
    }
}

pub fn needed_libraries(version: &Value) -> Vec<&Value> {
    let my_os = if cfg!(windows) { "windows" } else if cfg!(target_os = "macos") { "osx" } else { "linux" };

    version["libraries"].as_array().unwrap().iter().filter(|lib| {
        if lib["rules"].is_null() {
            return true;
        }
        let mut keep = false;
        for rule in lib["rules"].as_array().unwrap() {
            let os_ok = rule["os"]["name"].as_str().map_or(true, |n| n == my_os);
            if os_ok && rule["features"].is_null() {
                keep = rule["action"].as_str() == Some("allow");
            }
        }
        keep
    }).collect()
}

pub struct LibraryFile {
    pub path: String,
    pub url: String,
    pub size: Option<u64>,
}

pub fn library_file(lib: &Value) -> Option<LibraryFile> {
    if let Some(path) = lib["downloads"]["artifact"]["path"].as_str() {
        return Some(LibraryFile {
            path: path.to_string(),
            url: lib["downloads"]["artifact"]["url"].as_str()?.to_string(),
            size: lib["downloads"]["artifact"]["size"].as_u64(),
        });
    }

    let name = lib["name"].as_str()?;
    let mut parts = name.split(':');
    let group = parts.next()?;
    let artifact = parts.next()?;
    let version = parts.next()?;
    let path = format!(
        "{}/{artifact}/{version}/{artifact}-{version}.jar",
        group.replace('.', "/")
    );
    let base = lib["url"].as_str().unwrap_or("https://libraries.minecraft.net/");
    let base = if base.ends_with('/') {
        base.to_string()
    } else {
        format!("{base}/")
    };
    let url = format!("{base}{path}");

    Some(LibraryFile {
        path,
        url,
        size: None,
    })
}

fn classpath_of(mc_dir: &str, version: &Value, id: &str) -> String {
    let mut parts = vec![format!("{mc_dir}/versions/{id}/{id}.jar")];

    for lib in needed_libraries(version) {
        if let Some(file) = library_file(lib) {
            parts.push(format!("{mc_dir}/libraries/{}", file.path));
        }
    }

    let mut seen = std::collections::HashSet::new();
    parts.retain(|path| seen.insert(path.clone()));

    parts.join(path_separator())
}

fn path_separator() -> &'static str {
    if cfg!(windows) {
        ";"
    } else {
        ":"
    }
}

fn fill(template: &str, vars: &[(&str, String)]) -> String {
    let mut out = template.to_string();
    for (key, value) in vars {
        out = out.replace(*key, value);
    }
    out
}