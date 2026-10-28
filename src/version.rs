use std::path::Path;

use serde_json::{Map, Value};

pub struct ResolvedVersion {
    pub json: Value,
    pub jar_id: String,
    pub chain: Vec<String>,
}

pub struct InstanceInfo {
    pub loader: Option<String>,
    pub mc_version: String,
}

pub fn resolve(mc_dir: &str, id: &str) -> Result<ResolvedVersion, String> {
    let mut chain: Vec<Value> = Vec::new();
    let mut ids: Vec<String> = Vec::new();
    let mut current = id.to_string();

    loop {
        if ids.contains(&current) {
            return Err(format!("inheritsFrom cycle at {current}"));
        }

        let path = Path::new(mc_dir)
            .join("versions")
            .join(&current)
            .join(format!("{current}.json"));
        let text = std::fs::read_to_string(&path)
            .map_err(|e| format!("failed to read {}: {e}", path.display()))?;
        let json: Value = serde_json::from_str(&text)
            .map_err(|e| format!("failed to parse {}: {e}", path.display()))?;

        let parent = json["inheritsFrom"].as_str().map(|s| s.to_string());
        ids.push(current.clone());
        chain.push(json);

        match parent {
            Some(p) => current = p,
            None => break,
        }
    }

    let mut merged = chain.pop().unwrap();
    while let Some(child) = chain.pop() {
        merged = merge(merged, child);
    }

    let jar_id = find_jar_id(mc_dir, &ids);

    Ok(ResolvedVersion {
        json: merged,
        jar_id,
        chain: ids,
    })
}

pub fn instance_info(mc_dir: &str, id: &str) -> Result<InstanceInfo, String> {
    let resolved = resolve(mc_dir, id)?;
    let json = &resolved.json;

    let loader = detect_loader(json);
    let mc_version = json["clientVersion"]
        .as_str()
        .map(|value| value.to_string())
        .or_else(|| resolved.chain.last().cloned())
        .unwrap_or_else(|| id.to_string());

    Ok(InstanceInfo { loader, mc_version })
}

fn detect_loader(json: &Value) -> Option<String> {
    let main_class = json["mainClass"]
        .as_str()
        .unwrap_or_default()
        .to_ascii_lowercase();

    let libraries = json["libraries"]
        .as_array()
        .map(|items| items.as_slice())
        .unwrap_or(&[]);

    let has_lib = |needle: &str| {
        libraries.iter().any(|lib| {
            lib["name"]
                .as_str()
                .unwrap_or_default()
                .to_ascii_lowercase()
                .contains(needle)
        })
    };

    if main_class.contains("fabric") || has_lib("fabric-loader") {
        return Some(String::from("fabric"));
    }
    if has_lib("neoforge") {
        return Some(String::from("neoforge"));
    }
    if has_lib("net.minecraftforge") {
        return Some(String::from("forge"));
    }

    None
}

fn merge(parent: Value, child: Value) -> Value {
    let mut out: Map<String, Value> = parent.as_object().cloned().unwrap_or_default();

    for (key, value) in child.as_object().cloned().unwrap_or_default() {
        match key.as_str() {
            "libraries" => {
                let mut libs = out
                    .get("libraries")
                    .and_then(|v| v.as_array())
                    .cloned()
                    .unwrap_or_default();
                libs.extend(value.as_array().cloned().unwrap_or_default());
                out.insert(String::from("libraries"), Value::Array(libs));
            }
            "arguments" => {
                let mut merged_args = out
                    .get("arguments")
                    .cloned()
                    .unwrap_or_else(|| Value::Object(Map::new()));
                for kind in ["jvm", "game"] {
                    let mut args = merged_args
                        .get(kind)
                        .and_then(|v| v.as_array())
                        .cloned()
                        .unwrap_or_default();
                    args.extend(
                        value
                            .get(kind)
                            .and_then(|v| v.as_array())
                            .cloned()
                            .unwrap_or_default(),
                    );
                    merged_args[kind] = Value::Array(args);
                }
                out.insert(String::from("arguments"), merged_args);
            }
            "inheritsFrom" => {}
            _ => {
                out.insert(key, value);
            }
        }
    }

    Value::Object(out)
}

fn find_jar_id(mc_dir: &str, ids: &[String]) -> String {
    for id in ids.iter().rev() {
        let jar = Path::new(mc_dir)
            .join("versions")
            .join(id)
            .join(format!("{id}.jar"));
        if jar.exists() {
            return id.clone();
        }
    }

    ids.last().cloned().unwrap_or_default()
}
