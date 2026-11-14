mod account;
mod cli;
mod download;
mod gui;
mod i18n;
mod installer;
mod instance;
mod launch;
mod loader;
mod metadata;
mod microsoft;
mod modpack;
mod modrinth;
mod version;

use account::offline_uuid;
use cli::{AddCli, DownloadCli, ImportCli, LaunchCli, SearchCli};
use launch::{LaunchOptions, build_args};

fn absolute_path(path: &str) -> String {
    std::path::absolute(path)
        .map(|path| path.to_string_lossy().to_string())
        .unwrap_or_else(|_| path.to_string())
}

fn main() {
    if std::env::args().len() > 1 {
        match cli::parse() {
            cli::Command::Launch(c) => run_launch(&c),
            cli::Command::Download(c) => run_download(&c),
            cli::Command::Search(c) => run_search(&c),
            cli::Command::Add(c) => run_add(&c),
            cli::Command::Import(c) => run_import(&c),
            cli::Command::Login => run_login(),
            cli::Command::Logout => run_logout(),
        }
        return;
    }

    if let Err(error) = gui::run() {
        eprintln!("gui error: {error}");
        std::process::exit(1);
    }
}

fn run_login() {
    match microsoft::login() {
        Ok(()) => {}
        Err(e) => {
            eprintln!("login failed: {e}");
            std::process::exit(1);
        }
    }
}

fn run_logout() {
    match microsoft::logout() {
        Ok(()) => println!("logged out"),
        Err(e) => {
            eprintln!("logout failed: {e}");
            std::process::exit(1);
        }
    }
}

fn run_search(c: &SearchCli) {
    match modrinth::search(&c.query, &c.project_type, &c.loaders, &c.versions, c.limit) {
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
    let mc_dir = absolute_path(&c.mc_dir);
    let info = match version::instance_info(&mc_dir, &c.instance) {
        Ok(info) => info,
        Err(e) => {
            eprintln!("{}", e);
            std::process::exit(1);
        }
    };

    let loaders: Vec<String> = info.loader.into_iter().collect();
    let versions = vec![info.mc_version];

    match modrinth::install(
        &mc_dir,
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
    let mc_dir = absolute_path(&c.mc_dir);
    let origins = download::Origins {
        mojang: c.mirror.as_deref(),
        maven: c.maven.as_deref(),
    };

    let loader_name = c.loader.as_deref().map(str::to_ascii_lowercase);
    let java = launch::resolve_java(c.java.as_deref());

    let result = match loader_name.as_deref() {
        Some("fabric") => loader::install_fabric(
            &mc_dir,
            &c.version,
            c.loader_version.as_deref(),
            c.name.as_deref(),
            &origins,
        ),
        Some("forge") => loader::install_forge(
            &mc_dir,
            &c.version,
            c.loader_version.as_deref(),
            c.name.as_deref(),
            &java,
            &origins,
        ),
        Some("neoforge") => loader::install_neoforge(
            &mc_dir,
            &c.version,
            c.loader_version.as_deref(),
            c.name.as_deref(),
            &java,
            &origins,
        ),
        Some(other) => Err(format!(
            "unsupported loader: {other} (supported: fabric, forge, neoforge)"
        )),
        None => {
            let instance = c.name.clone().unwrap_or_else(|| c.version.clone());
            if let Err(e) = loader::validate_instance_id(&instance) {
                return eprintln!("{}", e);
            }
            match download::download_instance(&mc_dir, &instance, &c.version, &origins) {
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

fn run_import(c: &ImportCli) {
    let mc_dir = absolute_path(&c.mc_dir);
    let cache = std::path::Path::new(&mc_dir).join(".cache");
    if let Err(e) = std::fs::create_dir_all(&cache) {
        eprintln!("failed to create {}: {e}", cache.display());
        std::process::exit(1);
    }

    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default();
    let plain = download::Origins {
        mojang: None,
        maven: None,
    };

    let (path, source) = if let Some(file) = &c.file {
        (std::path::PathBuf::from(file), file.clone())
    } else if let Some(url) = &c.url {
        let mut name = url
            .split(['?', '#'])
            .next()
            .unwrap_or("modpack")
            .rsplit('/')
            .next()
            .unwrap_or("modpack");
        if !name.ends_with(".zip") && !name.ends_with(".mrpack") {
            name = "modpack.zip";
        }
        let stem = name.trim_end_matches(".mrpack").trim_end_matches(".zip");
        let path = cache.join(format!("{}-{stamp}.zip", loader::safe_name(stem)));
        if let Err(e) = download::fetch(url, &path, None, &plain) {
            eprintln!("failed to download modpack: {e}");
            std::process::exit(1);
        }
        (path, url.clone())
    } else if let Some(slug) = &c.modrinth {
        let path = cache.join(format!(
            "modpack-{}-{}.mrpack",
            loader::safe_name(slug),
            stamp
        ));
        match modrinth::download_modpack(slug, c.pack_version.as_deref(), &path) {
            Ok(version) => (path, format!("modrinth:{slug}@{version}")),
            Err(e) => {
                eprintln!("failed to download modpack: {e}");
                std::process::exit(1);
            }
        }
    } else {
        eprintln!("error: import requires --file, --url or --modrinth");
        return;
    };

    let origins = download::Origins {
        mojang: c.mirror.as_deref(),
        maven: c.maven.as_deref(),
    };
    let loader_name = c.loader.as_deref().map(str::to_ascii_lowercase);
    let loader = match loader_name.as_deref() {
        Some("fabric") => Some(modpack::LoaderKind::Fabric),
        Some("forge") => Some(modpack::LoaderKind::Forge),
        Some("neoforge") => Some(modpack::LoaderKind::NeoForge),
        Some(other) => {
            eprintln!("unsupported loader: {other}");
            return;
        }
        None => None,
    };

    let options = modpack::ImportOptions {
        mc_dir: &mc_dir,
        name: c.name.as_deref(),
        java: c.java.as_deref(),
        origins,
        with_optional: c.with_optional,
        dry_run: c.dry_run,
        source: &source,
        loader,
        loader_version: c.loader_version.as_deref(),
    };

    match modpack::import_file(&path, &options) {
        Ok(report) => {
            let action = if report.installed {
                "imported"
            } else {
                "dry run"
            };
            println!(
                "{} {} pack '{} {}'",
                action,
                report.provider.name(),
                report.name,
                report.version
            );
            println!("instance: {}", report.id);
            if !report.author.is_empty() {
                println!("author: {}", report.author);
            }
            if !report.description.is_empty() {
                println!("description: {}", report.description);
            }
            let loader = report
                .loader
                .as_ref()
                .map(|loader| {
                    format!(
                        "{} {}",
                        loader.kind.name(),
                        loader.version.as_deref().unwrap_or("")
                    )
                })
                .unwrap_or_else(|| String::from("vanilla"));
            println!("minecraft: {} / {}", report.game_version, loader);
            println!(
                "files: {}, overrides: {}, skipped optional: {}, size: {} MiB",
                report.files,
                report.overrides,
                report.excluded.len(),
                report.bytes / 1024 / 1024
            );
        }
        Err(e) => {
            eprintln!("failed to import modpack: {e}");
            std::process::exit(1);
        }
    }
}

fn run_launch(c: &LaunchCli) {
    let mc_dir = absolute_path(&c.mc_dir);
    let mc_dir = mc_dir.as_str();

    let resolved = version::resolve(mc_dir, &c.instance).unwrap_or_else(|error| {
        eprintln!("{}", error);
        std::process::exit(1);
    });
    let version = &resolved.json;

    let origins = download::Origins {
        mojang: c.mirror.as_deref(),
        maven: c.maven.as_deref(),
    };

    let java = launch::resolve_java(c.java.as_deref());
    if let Err(e) = launch::check_java(&java, version) {
        eprintln!("{}", e);
        return;
    }

    let (username, uuid, access_token) = match resolve_identity(c) {
        Ok(identity) => identity,
        Err(e) => {
            eprintln!("{e}");
            return;
        }
    };

    if let Err(e) = download::ensure_version_files(mc_dir, version, &resolved.jar_id, &origins) {
        eprintln!("failed to complete files: {}", e);
        return;
    }
    if let Err(e) = download::ensure_assets(mc_dir, version, &origins) {
        eprintln!("failed to complete assets: {}", e);
        return;
    }
    if let Err(e) = launch::prepare_natives(mc_dir, version) {
        eprintln!("failed to prepare natives: {}", e);
        return;
    }

    let game_dir = instance::game_dir(mc_dir, &c.instance, c.isolate);
    if let Err(e) = std::fs::create_dir_all(&game_dir) {
        eprintln!("failed to create game directory {}: {}", game_dir, e);
        return;
    }

    let opts = LaunchOptions {
        memory_mb: c.memory_mb,
        username,
        uuid,
        access_token,
        game_dir,
    };

    let args = build_args(mc_dir, version, &resolved.jar_id, &opts);

    let status = std::process::Command::new(&java)
        .args(&args)
        .current_dir(&opts.game_dir)
        .status();

    match status {
        Ok(status) if status.success() => {}
        Ok(status) => eprintln!("game exited with {status}"),
        Err(e) => eprintln!("failed to launch {java}: {e}"),
    }
}

fn resolve_identity(c: &LaunchCli) -> Result<(String, String, String), String> {
    if let Some(name) = &c.username {
        return Ok((name.clone(), offline_uuid(name), String::from("0")));
    }

    if c.offline {
        return Err(String::from("--offline requires --username"));
    }

    if let Some(mut account) = microsoft::load_account() {
        microsoft::ensure_fresh(&mut account)?;
        return Ok((account.name, account.uuid, account.access_token));
    }

    Err(String::from(
        "no Microsoft account: run `lyys_mc_launcher login` first, or pass --username <name> for offline mode",
    ))
}
