
use std::env;

pub enum Command {
    Launch(LaunchCli),
    Download(DownloadCli),
}

pub struct LaunchCli {
    pub mc_dir: String,
    pub instance: String,
    pub username: String,
    pub memory_mb: u32,
    pub isolate: bool,
}

pub struct DownloadCli {
    pub mc_dir: String,
    pub version: String,
    pub name: Option<String>,
    pub mirror: Option<String>,
}

const USAGE: &str = concat!(
    "usage:\n",
    "  launch: lyys_mc_launcher launch --dir <game_dir> --name <instance> --username <name> [--memory <MB>] [--isolate]\n",
    "  download: lyys_mc_launcher download --dir <game_dir> --version <version> [--name <instance>] [--mirror <url>]",
);

pub fn parse() -> Command {
    let args: Vec<String> = env::args().collect();
    let rest = &args[args.len().min(2)..];

    match args.get(1).map(String::as_str) {
        Some("launch") => Command::Launch(parse_launch(rest)),
        Some("download") => Command::Download(parse_download(rest)),
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

    for w in rest.windows(2) {
        match w[0].as_str() {
            "--dir" => mc_dir = Some(w[1].clone()),
            "--name" => instance = Some(w[1].clone()),
            "--username" => username = Some(w[1].clone()),
            "--memory" => memory_mb = w[1].parse().unwrap_or(memory_mb),
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
    }
}

fn parse_download(rest: &[String]) -> DownloadCli {
    let mut mc_dir: Option<String> = None;
    let mut version: Option<String> = None;
    let mut name: Option<String> = None;
    let mut mirror: Option<String> = None;

    for w in rest.windows(2) {
        match w[0].as_str() {
            "--dir" => mc_dir = Some(w[1].clone()),
            "--version" => version = Some(w[1].clone()),
            "--name" => name = Some(w[1].clone()),
            "--mirror" => mirror = Some(w[1].clone()),
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
    }
}

fn check_dir(mc_dir: &str) {
    if !std::path::Path::new(mc_dir).is_dir() {
        eprintln!("error: game directory does not exist: {mc_dir}");
        std::process::exit(1);
    }
}