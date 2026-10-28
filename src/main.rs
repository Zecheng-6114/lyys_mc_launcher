mod account;
mod cli;
mod download;
mod launch;
mod loader;
mod modrinth;
mod version;

use account::offline_uuid;
use cli::{AddCli, DownloadCli, LaunchCli, SearchCli};
use launch::{build_args, LaunchOptions};

fn main() {
    match cli::parse() {
        cli::Command::Launch(c) => run_launch(&c),
        cli::Command::Download(c) => run_download(&c),
        cli::Command::Search(c) => run_search(&c),
        cli::Command::Add(c) => run_add(&c),
    }
}

fn run_search(c: &SearchCli) {
    match modrinth::search(
        &c.query,
        &c.project_type,
        &c.loaders,
        &c.versions,
        c.limit,
    ) {
        Ok(hits) if hits.is_empty() => println!("no results"),
        Ok(hits) => {
            for hit in hits {
                println!(
                    "{}\t{}\t{}\t{}\t{}",
                    hit.slug, hit.project_type, hit.downloads, hit.title, hit.description
                );
            }
        }
        Err(e) => {
            eprintln!("search failed: {}", e);
            std::process::exit(1);
        }
    }
}

fn run_add(c: &AddCli) {
    let info = match version::instance_info(&c.mc_dir, &c.instance) {
        Ok(info) => info,
        Err(e) => {
            eprintln!("{}", e);
            std::process::exit(1);
        }
    };

    let loaders: Vec<String> = info.loader.into_iter().collect();
    let versions = vec![info.mc_version];

    match modrinth::install(
        &c.mc_dir,
        &c.instance,
        &c.slug,
        &c.project_type,
        &loaders,
        &versions,
        c.with_optional,
    ) {
        Ok(report) => {
            println!("installed {} file(s):", report.files.len());
            for file in &report.files {
                println!("{}", file);
            }

            if !report.optional.is_empty() {
                println!();
                println!("optional dependencies:");
                for dep in &report.optional {
                    let state = if dep.installed {
                        "installed"
                    } else {
                        "skipped, use --with-optional"
                    };
                    println!("  {}  ({})", dep.name, state);
                }
            }
        }
        Err(e) => {
            eprintln!("failed to add mod: {}", e);
            std::process::exit(1);
        }
    }
}

fn run_download(c: &DownloadCli) {
    let origins = download::Origins {
        mojang: c.mirror.as_deref(),
        maven: c.maven.as_deref(),
    };

    let result = match c.loader.as_deref() {
        Some("fabric") => loader::install_fabric(
            &c.mc_dir,
            &c.version,
            c.loader_version.as_deref(),
            c.name.as_deref(),
            &origins,
        ),
        Some(other) => Err(format!("unsupported loader: {other} (supported: fabric)")),
        None => {
            let instance = c.name.clone().unwrap_or_else(|| c.version.clone());
            match download::download_instance(&c.mc_dir, &instance, &c.version, &origins) {
                Ok(()) => Ok(instance),
                Err(e) => Err(e),
            }
        }
    };

    match result {
        Ok(id) => println!("instance {} installed", id),
        Err(e) => {
            eprintln!("failed to install: {}", e);
            std::process::exit(1);
        }
    }
}

fn run_launch(c: &LaunchCli) {
    let mc_dir = c.mc_dir.as_str();

    let resolved = version::resolve(mc_dir, &c.instance).unwrap_or_else(|error| {
        eprintln!("{}", error);
        std::process::exit(1);
    });
    let version = &resolved.json;

    let origins = download::Origins {
        mojang: c.mirror.as_deref(),
        maven: c.maven.as_deref(),
    };

    if let Err(e) = download::ensure_version_files(mc_dir, version, &resolved.jar_id, &origins) {
        eprintln!("failed to complete files: {}", e);
        return;
    }
    if let Err(e) = download::ensure_assets(mc_dir, version, &origins) {
        eprintln!("failed to complete assets: {}", e);
        return;
    }

    let game_dir = if c.isolate {
        format!("{mc_dir}/versions/{}", c.instance)
    } else {
        c.mc_dir.clone()
    };
    if let Err(e) = std::fs::create_dir_all(&game_dir) {
        eprintln!("failed to create game directory {}: {}", game_dir, e);
        return;
    }

    let opts = LaunchOptions {
        memory_mb: c.memory_mb,
        username: c.username.clone(),
        uuid: offline_uuid(&c.username),
        access_token: String::from("0"),
        game_dir,
    };

    let args = build_args(mc_dir, version, &resolved.jar_id, &opts);

    let java = c.java.clone().unwrap_or_else(|| String::from("java"));

    std::process::Command::new(java)
        .args(&args)
        .current_dir(&opts.game_dir)
        .status()
        .unwrap();
}
