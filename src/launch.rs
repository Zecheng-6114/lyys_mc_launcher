
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

fn classpath_of(mc_dir: &str, version: &Value, id: &str) -> String {
    let mut parts = vec![format!("{mc_dir}/versions/{id}/{id}.jar")];

    for lib in needed_libraries(version) {
        if let Some(path) = lib["downloads"]["artifact"]["path"].as_str() {
            parts.push(format!("{mc_dir}/libraries/{path}"));
        }
    }

    parts.join(if cfg!(windows) { ";" } else { ":" })
}

fn fill(template: &str, vars: &[(&str, String)]) -> String {
    let mut out = template.to_string();
    for (key, value) in vars {
        out = out.replace(*key, value);
    }
    out
}