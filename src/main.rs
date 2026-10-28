
mod account;
mod cli;
mod download;
mod launch;

use account::offline_uuid;
use cli::{DownloadCli, LaunchCli};
use launch::{build_args, LaunchOptions};

fn main() {
    match cli::parse() {
        cli::Command::Launch(c) => run_launch(&c),
        cli::Command::Download(c) => run_download(&c),
    }
}

fn run_download(c: &DownloadCli) {
    let origins = download::Origins {
        mojang: c.mirror.as_deref(),
        maven: c.maven.as_deref(),
    };
    let instance = c.name.clone().unwrap_or_else(|| c.version.clone());
    match download::download_instance(&c.mc_dir, &instance, &c.version, &origins) {
        Ok(()) => println!("instance {instance} downloaded"),
        Err(e) => {
            eprintln!("failed to download instance: {e}");
            std::process::exit(1);
        }
    }
}

fn run_launch(c: &LaunchCli) {
    let mc_dir = c.mc_dir.as_str();

    let version = match download::load_version_json(mc_dir, &c.instance) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("{e}");
            return;
        }
    };

    let origins = download::Origins {
        mojang: c.mirror.as_deref(),
        maven: c.maven.as_deref(),
    };

    if let Err(e) = download::ensure_version_files(mc_dir, &version, &origins) {
        eprintln!("failed to complete files: {e}");
        return;
    }
    if let Err(e) = download::ensure_assets(mc_dir, &version, &origins) {
        eprintln!("failed to complete assets: {e}");
        return;
    }

    let game_dir = if c.isolate {
        format!("{mc_dir}/versions/{}", c.instance)
    } else {
        c.mc_dir.clone()
    };
    if let Err(e) = std::fs::create_dir_all(&game_dir) {
        eprintln!("failed to create game directory {game_dir}: {e}");
        return;
    }

    let opts = LaunchOptions {
        memory_mb: c.memory_mb,
        username: c.username.clone(),
        uuid: offline_uuid(&c.username),
        access_token: String::from("0"),
        game_dir,
    };

    let args = build_args(mc_dir, &version, &opts);

    let java = c.java.clone().unwrap_or_else(|| String::from("java"));

    std::process::Command::new(java)
        .args(&args)
        .current_dir(&opts.game_dir)
        .status()
        .unwrap();
}