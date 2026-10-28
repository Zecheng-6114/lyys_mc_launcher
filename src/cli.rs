
use std::env;

pub enum Command {
    Launch(LaunchCli),
    Download(DownloadCli),
    Search(SearchCli),
    Add(AddCli),
}

pub struct LaunchCli {
    pub mc_dir: String,
    pub instance: String,
    pub username: String,
    pub memory_mb: u32,
    pub isolate: bool,
    pub mirror: Option<String>,
    pub maven: Option<String>,
    pub java: Option<String>,
}

pub struct SearchCli {
    pub query: String,
    pub project_type: String,
    pub loaders: Vec<String>,
    pub versions: Vec<String>,
    pub limit: u32,
}

pub struct AddCli {
    pub mc_dir: String,
    pub instance: String,
    pub slug: String,
    pub project_type: String,
    pub with_optional: bool,
}

pub struct DownloadCli {
    pub mc_dir: String,
    pub version: String,
    pub name: Option<String>,
    pub mirror: Option<String>,
    pub maven: Option<String>,
    pub loader: Option<String>,
    pub loader_version: Option<String>,
}

const USAGE: &str = concat!(
    "usage:\n",
    "  launch: lyys_mc_launcher launch --dir <game_dir> --name <instance> --username <name> [--memory <MB>] [--isolate] [--mirror <url>] [--maven <url>] [--java <path>]\n",
    "  download: lyys_mc_launcher download --dir <game_dir> --version <version> [--loader <fabric>] [--loader-version <v>] [--name <instance>] [--mirror <url>] [--maven <url>]\n",
    "  search: lyys_mc_launcher search --query <text> --type <mod|modpack|resourcepack|shader|datapack> [--loader <a,b>] [--version <x,y>] [--limit <n>]\n",
    "  add:    lyys_mc_launcher add --dir <game_dir> --name <instance> --slug <slug> --type <kind> [--with-optional]",
);

pub fn parse() -> Command {
    let args: Vec<String> = env::args().collect();
    let rest = &args[args.len().min(2)..];

    match args.get(1).map(String::as_str) {
        Some("launch") => Command::Launch(parse_launch(rest)),
        Some("download") => Command::Download(parse_download(rest)),
        Some("search") => Command::Search(parse_search(rest)),
        Some("add") => Command::Add(parse_add(rest)),
        _ => {
            println!("{USAGE}");
            std::process::exit(0);
        }
    }
}

fn parse_launch(rest: &[String]) -> LaunchCli {
    let mut mc_dir: Option<String> = None;
    let mut instance: Option<String> = None;
    let mut username: Option<String> = None;
    let mut memory_mb = 4096;
    let mut mirror: Option<String> = None;
    let mut maven: Option<String> = None;
    let mut java: Option<String> = None;

    for w in rest.windows(2) {
        match w[0].as_str() {
            "--dir" => mc_dir = Some(w[1].clone()),
            "--name" => instance = Some(w[1].clone()),
            "--username" => username = Some(w[1].clone()),
            "--memory" => memory_mb = w[1].parse().unwrap_or(memory_mb),
            "--mirror" => mirror = Some(w[1].clone()),
            "--maven" => maven = Some(w[1].clone()),
            "--java" => java = Some(w[1].clone()),
            _ => {}
        }
    }
    let isolate = rest.iter().any(|a| a == "--isolate");

    let mut missing = Vec::new();
    if mc_dir.is_none() {
        missing.push("--dir");
    }
    if instance.is_none() {
        missing.push("--name");
    }
    if username.is_none() {
        missing.push("--username");
    }
    if !missing.is_empty() {
        eprintln!("error: missing required argument(s): {}", missing.join(", "));
        eprintln!("{USAGE}");
        std::process::exit(1);
    }

    let mc_dir = mc_dir.unwrap();
    check_dir(&mc_dir);

    LaunchCli {
        mc_dir,
        instance: instance.unwrap(),
        username: username.unwrap(),
        memory_mb,
        isolate,
        mirror,
        maven,
        java,
    }
}

fn parse_download(rest: &[String]) -> DownloadCli {
    let mut mc_dir: Option<String> = None;
    let mut version: Option<String> = None;
    let mut name: Option<String> = None;
    let mut mirror: Option<String> = None;
    let mut maven: Option<String> = None;
    let mut loader: Option<String> = None;
    let mut loader_version: Option<String> = None;

    for w in rest.windows(2) {
        match w[0].as_str() {
            "--dir" => mc_dir = Some(w[1].clone()),
            "--version" => version = Some(w[1].clone()),
            "--name" => name = Some(w[1].clone()),
            "--mirror" => mirror = Some(w[1].clone()),
            "--maven" => maven = Some(w[1].clone()),
            "--loader" => loader = Some(w[1].clone()),
            "--loader-version" => loader_version = Some(w[1].clone()),
            _ => {}
        }
    }

    let (Some(mc_dir), Some(version)) = (mc_dir, version) else {
        eprintln!("error: download requires --dir and --version");
        eprintln!("{USAGE}");
        std::process::exit(1);
    };
    check_dir(&mc_dir);

    DownloadCli {
        mc_dir,
        version,
        name,
        mirror,
        maven,
        loader,
        loader_version,
    }
}

fn parse_search(rest: &[String]) -> SearchCli {
    let mut query: Option<String> = None;
    let mut project_type: Option<String> = None;
    let mut loaders: Vec<String> = Vec::new();
    let mut versions: Vec<String> = Vec::new();
    let mut limit = 10;

    for w in rest.windows(2) {
        match w[0].as_str() {
            "--query" => query = Some(w[1].clone()),
            "--type" => project_type = Some(w[1].clone()),
            "--loader" => loaders.extend(split_list(&w[1])),
            "--version" => versions.extend(split_list(&w[1])),
            "--limit" => limit = w[1].parse().unwrap_or(limit),
            _ => {}
        }
    }

    let (Some(query), Some(project_type)) = (query, project_type) else {
        eprintln!("error: search requires --query and --type");
        eprintln!("{USAGE}");
        std::process::exit(1);
    };

    SearchCli {
        query,
        project_type,
        loaders,
        versions,
        limit,
    }
}

fn parse_add(rest: &[String]) -> AddCli {
    let mut mc_dir: Option<String> = None;
    let mut instance: Option<String> = None;
    let mut slug: Option<String> = None;
    let mut project_type: Option<String> = None;

    for w in rest.windows(2) {
        match w[0].as_str() {
            "--dir" => mc_dir = Some(w[1].clone()),
            "--name" => instance = Some(w[1].clone()),
            "--slug" => slug = Some(w[1].clone()),
            "--type" => project_type = Some(w[1].clone()),
            _ => {}
        }
    }
    let with_optional = rest.iter().any(|arg| arg == "--with-optional");

    let mut missing = Vec::new();
    if mc_dir.is_none() {
        missing.push("--dir");
    }
    if instance.is_none() {
        missing.push("--name");
    }
    if slug.is_none() {
        missing.push("--slug");
    }
    if project_type.is_none() {
        missing.push("--type");
    }
    if !missing.is_empty() {
        eprintln!("error: missing required argument(s): {}", missing.join(", "));
        eprintln!("{USAGE}");
        std::process::exit(1);
    }

    let mc_dir = mc_dir.unwrap();
    check_dir(&mc_dir);

    AddCli {
        mc_dir,
        instance: instance.unwrap(),
        slug: slug.unwrap(),
        project_type: project_type.unwrap(),
        with_optional,
    }
}

fn split_list(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(|item| item.trim().to_string())
        .filter(|item| !item.is_empty())
        .collect()
}

fn check_dir(mc_dir: &str) {
    if !std::path::Path::new(mc_dir).is_dir() {
        eprintln!("error: game directory does not exist: {mc_dir}");
        std::process::exit(1);
    }
}
