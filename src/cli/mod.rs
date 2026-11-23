use std::path::PathBuf;
use std::sync::Arc;

use clap::{Args, Parser, Subcommand};
use mc_launcher_core::loader::common::{LoaderSpec, LoaderVersion};
use mc_launcher_core::utils::get_java_executable;
use thiserror::Error;

use crate::core::auth::{AccountSession, Auth, AuthError, SessionState};
use crate::core::config::{self, ConfigError, InstanceConfig, LauncherConfig};
use crate::core::i18n::{self, t, tf};
use crate::core::install;
use crate::core::java;
use crate::core::launch::{self, LaunchAccount, LaunchError, LaunchRequest};
use crate::core::modpack::{self, InstallOptions, PackProgress};
use crate::core::modrinth::{self, ModrinthError, ModSearchRequest};
use crate::gui;

const DEFAULT_MINECRAFT_DIR: &str = ".minecraft";

#[derive(Debug, Error)]
pub enum CliError {
    #[error(transparent)]
    Auth(#[from] AuthError),
    #[error(transparent)]
    Launch(#[from] LaunchError),
    #[error(transparent)]
    Modrinth(#[from] ModrinthError),
    #[error(transparent)]
    Config(#[from] ConfigError),
    #[error(transparent)]
    Core(#[from] mc_launcher_core::LauncherError),
    #[error("the game exited with {0}")]
    Game(std::process::ExitStatus),
    #[error("install task failed: {0}")]
    Task(String),
    #[error("unknown loader {0}, expected fabric, quilt, forge or neoforge")]
    UnknownLoader(String),
    #[error("{0}")]
    Usage(String),
    #[error("gui failed: {0}")]
    Gui(String),
}

#[derive(Parser)]
#[command(version, about = "LYYS MC Launcher")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    Account(AccountArgs),
    Install(InstallArgs),
    Launch(LaunchArgs),
    /// List the Java runtimes found on this machine.
    Java,
    Search(SearchArgs),
    Download(DownloadArgs),
    Modpack(ModpackArgs),
    Config(ConfigArgs),
    Gui,
}

#[derive(Args)]
struct AccountArgs {
    #[command(subcommand)]
    action: AccountAction,
}

#[derive(Subcommand)]
enum AccountAction {
    Login,
    Offline { name: String },
    List,
    Use { uuid: String },
    Remove { uuid: String },
    Current,
}

#[derive(Args)]
struct InstallArgs {
    version: String,
    #[arg(long)]
    minecraft_dir: Option<PathBuf>,
    /// Mod loader to install: fabric, quilt, forge or neoforge.
    #[arg(long)]
    loader: Option<String>,
    /// Exact loader version; defaults to the latest compatible one.
    #[arg(long, requires = "loader")]
    loader_version: Option<String>,
    /// Java executable used by the Forge/NeoForge installer (defaults to the configured one).
    #[arg(long)]
    java: Option<PathBuf>,
}

#[derive(Args)]
struct LaunchArgs {
    version: String,
    #[arg(long)]
    minecraft_dir: Option<PathBuf>,
    #[arg(long)]
    game_dir: Option<PathBuf>,
    #[arg(long)]
    java: Option<PathBuf>,
    /// Override the configured version isolation for this launch.
    #[arg(long, conflicts_with_all = ["game_dir", "no_isolate"])]
    isolate: bool,
    /// Disable version isolation for this launch.
    #[arg(long, conflicts_with = "game_dir")]
    no_isolate: bool,
}

#[derive(Args)]
struct ConfigArgs {
    #[command(subcommand)]
    action: ConfigAction,
}

#[derive(Subcommand)]
enum ConfigAction {
    /// Print the configuration.
    Show,
    /// Update the global configuration.
    Set {
        #[arg(long)]
        minecraft_dir: Option<PathBuf>,
        #[arg(long)]
        java: Option<PathBuf>,
        /// Enable version isolation.
        #[arg(long, conflicts_with = "no_isolate")]
        isolate: bool,
        /// Disable version isolation.
        #[arg(long)]
        no_isolate: bool,
    },
    /// Use the global configuration or override it for one instance.
    Instance {
        version: String,
        /// Fall back to the global configuration.
        #[arg(long, conflicts_with_all = ["isolate", "no_isolate", "java"])]
        global: bool,
        #[arg(long, conflicts_with = "no_isolate")]
        isolate: bool,
        #[arg(long)]
        no_isolate: bool,
        #[arg(long)]
        java: Option<PathBuf>,
    },
}

#[derive(Args)]
struct SearchArgs {
    /// Search query; leave empty to list popular projects.
    #[arg(long)]
    query: Option<String>,
    /// Project type: mod, resourcepack, shader or datapack.
    #[arg(long = "type", value_enum, default_value = "mod")]
    kind: ContentType,
    /// Only show projects for this Minecraft version.
    #[arg(long)]
    minecraft: Option<String>,
    /// Only show mods for this loader: fabric, quilt, forge or neoforge.
    #[arg(long)]
    loader: Option<String>,
}

#[derive(Args)]
struct DownloadArgs {
    /// Modrinth project slug or id.
    project: String,
    /// Project type: mod, resourcepack, shader or datapack.
    #[arg(long = "type", value_enum, default_value = "mod")]
    kind: ContentType,
    /// Minecraft version used to pick a compatible file.
    #[arg(long)]
    minecraft: String,
    /// Mod loader; required when installing mods.
    #[arg(long)]
    loader: Option<String>,
    #[arg(long)]
    minecraft_dir: Option<PathBuf>,
    /// Instance whose isolation setting decides the game directory.
    #[arg(long)]
    instance: Option<String>,
    /// Write the file into this directory instead of the default location.
    #[arg(long)]
    target: Option<PathBuf>,
    /// Save name used for data packs.
    #[arg(long)]
    world: Option<String>,
}

#[derive(Args)]
struct ModpackArgs {
    #[command(subcommand)]
    action: ModpackAction,
}

#[derive(Subcommand)]
enum ModpackAction {
    /// Install a modpack from a .mrpack, CurseForge or server pack archive.
    Install(ModpackInstallArgs),
    /// Print what a modpack archive contains without installing it.
    Info { file: PathBuf },
}

#[derive(Args)]
struct ModpackInstallArgs {
    /// Path to the modpack archive.
    file: PathBuf,
    #[arg(long)]
    minecraft_dir: Option<PathBuf>,
    /// Java used by the Forge/NeoForge installer.
    #[arg(long)]
    java: Option<PathBuf>,
    /// Also install files the pack marks as optional.
    #[arg(long)]
    with_optional: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
enum ContentType {
    Mod,
    Resourcepack,
    Shader,
    Datapack,
}

impl From<ContentType> for modrinth::ProjectType {
    fn from(kind: ContentType) -> Self {
        match kind {
            ContentType::Mod => Self::Mod,
            ContentType::Resourcepack => Self::ResourcePack,
            ContentType::Shader => Self::ShaderPack,
            ContentType::Datapack => Self::DataPack,
        }
    }
}

pub async fn run() -> Result<(), CliError> {
    let language = LauncherConfig::load()
        .ok()
        .and_then(|config| i18n::Language::from_code(&config.language))
        .unwrap_or_else(i18n::Language::detect);
    i18n::set_language(language);
    match Cli::parse().command {
        Some(Command::Account(args)) => run_account(args.action).await,
        Some(Command::Install(args)) => run_install(args).await,
        Some(Command::Launch(args)) => run_launch(args).await,
        Some(Command::Java) => run_java_list(),
        Some(Command::Search(args)) => run_search(args).await,
        Some(Command::Download(args)) => run_download(args).await,
        Some(Command::Modpack(args)) => run_modpack(args).await,
        Some(Command::Config(args)) => run_config(args.action),
        Some(Command::Gui) | None => gui::run().map_err(CliError::Gui),
    }
}

async fn run_account(action: AccountAction) -> Result<(), CliError> {
    let auth = Auth::new()?;
    match action {
        AccountAction::Login => {
            let session = auth
                .login(|prompt| {
                    println!(
                        "{}",
                        tf(
                            "cli.login.open",
                            &[
                                ("url", prompt.verification_uri.as_str()),
                                ("code", prompt.user_code.as_str()),
                            ],
                        )
                    );
                    let minutes = (prompt.expires_in / 60).to_string();
                    println!(
                        "{}",
                        tf("cli.login.code_valid", &[("minutes", minutes.as_str())])
                    );
                })
                .await?;
            println!(
                "{}",
                tf(
                    "cli.login.logged_in",
                    &[
                        ("name", session.profile.name.as_str()),
                        ("uuid", session.profile.id.as_str()),
                    ],
                )
            );
        }
        AccountAction::List => print_accounts(&auth)?,
        AccountAction::Offline { name } => {
            let session = auth.add_offline(&name)?;
            println!(
                "{}",
                tf(
                    "cli.account.offline_added",
                    &[
                        ("name", session.profile.name.as_str()),
                        ("uuid", session.profile.id.as_str()),
                    ],
                )
            );
        }
        AccountAction::Use { uuid } => {
            auth.set_active(&uuid)?;
            println!("{}", tf("cli.account.active_set", &[("account", uuid.as_str())]));
        }
        AccountAction::Remove { uuid } => {
            auth.remove(&uuid)?;
            println!("{}", tf("cli.account.removed", &[("account", uuid.as_str())]));
        }
        AccountAction::Current => {
            let session = auth.refresh_current().await?;
            println!("{}", account_line("*", &session));
        }
    }
    Ok(())
}

async fn run_install(args: InstallArgs) -> Result<(), CliError> {
    let config = LauncherConfig::load()?;
    let minecraft_dir = resolve_minecraft_dir(args.minecraft_dir, &config)?;
    let version_id = args.version;
    let java = args
        .java
        .or_else(|| configured_java(&config.java_path))
        .or_else(|| get_java_executable().map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("java"));
    let loader = match args.loader {
        Some(name) => Some(parse_loader(&name, args.loader_version)?),
        None => None,
    };
    // 核心库的下载走阻塞式 HTTP 客户端，放在阻塞线程池里执行，避免和异步运行时互相卡住
    let install_dir = minecraft_dir.clone();
    let install_version = version_id.clone();
    tokio::task::spawn_blocking(move || match loader {
        Some(loader) => {
            let mut printed = false;
            let mut reporter = |progress: install::InstallProgress| {
                printed = true;
                let downloaded = progress.downloaded.to_string();
                let skipped = progress.skipped.to_string();
                print!(
                    "\r{}",
                    tf(
                        "install.summary",
                        &[("downloaded", downloaded.as_str()), ("skipped", skipped.as_str())],
                    )
                );
                let _ = std::io::Write::flush(&mut std::io::stdout());
            };
            let result = install::install_loader_with_reporter(
                &install_dir,
                &install_version,
                loader,
                &java,
                &mut reporter,
            );
            if printed {
                println!();
            }
            result
        }
        None => install::install(&install_dir, &install_version),
    })
    .await
    .map_err(|error| CliError::Task(error.to_string()))??;
    let directory = minecraft_dir.display().to_string();
    println!(
        "{}",
        tf(
            "install.installed_into",
            &[("name", version_id.as_str()), ("dir", directory.as_str())],
        )
    );
    Ok(())
}

fn parse_loader(name: &str, version: Option<String>) -> Result<LoaderSpec, CliError> {
    let version = match version {
        Some(version) if !version.trim().is_empty() => LoaderVersion::Exact(version),
        _ => LoaderVersion::Latest,
    };
    match name.to_ascii_lowercase().as_str() {
        "fabric" => Ok(LoaderSpec::Fabric { version }),
        "quilt" => Ok(LoaderSpec::Quilt { version }),
        "forge" => Ok(LoaderSpec::Forge { version }),
        "neoforge" => Ok(LoaderSpec::NeoForge { version }),
        other => Err(CliError::UnknownLoader(other.to_owned())),
    }
}

async fn run_launch(args: LaunchArgs) -> Result<(), CliError> {
    let config = LauncherConfig::load()?;
    let minecraft_dir = resolve_minecraft_dir(args.minecraft_dir, &config)?;
    let java = match args.java {
        Some(java) => Some(java),
        None => configured_java(config.effective_java(&args.version)),
    };
    let isolate = if args.isolate {
        true
    } else if args.no_isolate {
        false
    } else {
        config.effective_isolate(&args.version)
    };

    let auth = Auth::new()?;
    let session = auth.refresh_current().await?;
    let token;
    let account = if session.state == SessionState::Offline {
        LaunchAccount::Offline(&session.profile)
    } else {
        token = auth.current_game_token().await?;
        LaunchAccount::Microsoft(&token)
    };
    let mut child = launch::launch(LaunchRequest {
        minecraft_dir: &minecraft_dir,
        version_id: &args.version,
        account,
        java: java.as_deref(),
        java_candidates: &config.java_versions,
        game_dir: args.game_dir.as_deref(),
        isolated: isolate,
    })?;
    println!(
        "{}",
        tf(
            "cli.launch.launched",
            &[
                ("name", args.version.as_str()),
                ("pid", child.id().to_string().as_str()),
            ],
        )
    );
    // 子进程继承终端输出，不等到它退出就交还 shell 会把游戏日志和提示符搅在一起
    let status = child.wait().map_err(AuthError::from)?;
    if !status.success() {
        return Err(CliError::Game(status));
    }
    Ok(())
}

/// Print the Java runtimes found on this machine, marking the configured one.
fn run_java_list() -> Result<(), CliError> {
    let config = LauncherConfig::load()?;
    let minecraft_dir = resolve_minecraft_dir(None, &config)?;
    let selected = config.java_path.trim();
    let runtimes = java::discover(&minecraft_dir, &config.java_versions);
    println!("{}", t("settings.java.installed"));
    if runtimes.is_empty() {
        println!("{}", t("cli.java.none"));
        return Ok(());
    }
    for runtime in &runtimes {
        let path = runtime.executable.display().to_string();
        let marker = if path == selected {
            format!(" {}", t("cli.java.selected"))
        } else {
            String::new()
        };
        println!("Java {}{}  {}", runtime.version, marker, path);
    }
    Ok(())
}

async fn run_search(args: SearchArgs) -> Result<(), CliError> {
    let kind = modrinth::ProjectType::from(args.kind);
    let loader = if kind == modrinth::ProjectType::Mod {
        args.loader.as_deref()
    } else {
        None
    };
    let result = modrinth::search(ModSearchRequest {
        query: args.query.as_deref().unwrap_or(""),
        project_type: kind,
        game_version: args.minecraft.as_deref(),
        loader,
        offset: 0,
    })
    .await?;
    if result.hits.is_empty() {
        println!("{}", t("cli.search.none"));
        return Ok(());
    }
    for hit in &result.hits {
        let downloads = hit.downloads.to_string();
        println!(
            "{}",
            tf(
                "cli.search.hit",
                &[("title", hit.title.as_str()), ("downloads", downloads.as_str())],
            )
        );
        println!(
            "  https://modrinth.com/{}/{}",
            kind.url_segment(),
            hit.slug
        );
    }
    println!();
    let total_hits = result.total_hits.to_string();
    let shown = result.hits.len().to_string();
    println!(
        "{}",
        tf("cli.search.results", &[("total", total_hits.as_str()), ("shown", shown.as_str())])
    );
    Ok(())
}

async fn run_download(args: DownloadArgs) -> Result<(), CliError> {
    let kind = modrinth::ProjectType::from(args.kind);
    let game_version = args.minecraft.trim();
    if game_version.is_empty() {
        return Err(CliError::Usage(
            t("cli.error.minecraft_empty").to_owned(),
        ));
    }
    let loader = args.loader.as_deref().map(str::trim).unwrap_or("");
    if kind == modrinth::ProjectType::Mod && loader.is_empty() {
        return Err(CliError::Usage(
            t("cli.error.mods_loader").to_owned(),
        ));
    }

    let target = match args.target {
        Some(dir) => dir,
        None => {
            let config = LauncherConfig::load()?;
            let base = resolve_minecraft_dir(args.minecraft_dir, &config)?;
            let isolate = args
                .instance
                .as_deref()
                .map_or(config.isolate, |instance| config.effective_isolate(instance));
            let game_dir = match args.instance.as_deref() {
                Some(instance) if isolate => base.join("versions").join(instance),
                _ => base,
            };
            match kind {
                modrinth::ProjectType::Mod => game_dir.join("mods"),
                modrinth::ProjectType::ResourcePack => game_dir.join("resourcepacks"),
                modrinth::ProjectType::ShaderPack => game_dir.join("shaderpacks"),
                modrinth::ProjectType::DataPack => {
                    let world = args.world.as_deref().map(str::trim).unwrap_or("");
                    if world.is_empty() {
                        return Err(CliError::Usage(
                            t("cli.error.datapack_world").to_owned(),
                        ));
                    }
                    game_dir.join("saves").join(world).join("datapacks")
                }
                modrinth::ProjectType::Modpack => {
                    return Err(CliError::Usage(
                        t("cli.error.modpack_command").to_owned(),
                    ));
                }
            }
        }
    };

    let outcome = match kind {
        modrinth::ProjectType::Mod => {
            modrinth::install_with_dependencies(&args.project, game_version, loader, &target).await?
        }
        _ => modrinth::install_content(&args.project, game_version, &target).await?,
    };
    for installed in &outcome.installed {
        println!(
            "{}",
            tf(
                "cli.install.installed_detail",
                &[
                    ("name", installed.filename.as_str()),
                    ("version", installed.version_number.as_str()),
                ],
            )
        );
    }
    let directory = target.display().to_string();
    println!("{}", tf("common.saved_to", &[("path", directory.as_str())]));
    if !outcome.skipped.is_empty() {
        let skipped = outcome.skipped.join(", ");
        println!("{}", tf("content.missing_dependencies", &[("list", skipped.as_str())]));
    }
    Ok(())
}

async fn run_modpack(args: ModpackArgs) -> Result<(), CliError> {
    match args.action {
        ModpackAction::Install(args) => run_modpack_install(args).await,
        ModpackAction::Info { file } => {
            let pack = modpack::inspect(&file)
                .await
                .map_err(|error| CliError::Task(error.to_string()))?;
            println!(
                "{}",
                tf(
                    "cli.modpack.title",
                    &[("name", pack.name.as_str()), ("version", pack.version.as_str())]
                )
            );
            println!("{}", tf("cli.modpack.provider", &[("provider", pack.provider.name())]));
            println!("{}", tf("cli.modpack.minecraft", &[("version", pack.game_version.as_str())]));
            if let Some(loader) = &pack.loader {
                println!(
                    "{}",
                    tf(
                        "cli.modpack.loader",
                        &[("loader", modpack::describe_loader(loader).as_str())],
                    )
                );
            }
            if !pack.author.trim().is_empty() {
                println!("{}", tf("cli.modpack.author", &[("author", pack.author.as_str())]));
            }
            if !pack.description.trim().is_empty() {
                println!("{}", pack.description);
            }
            let files = pack.installable_files(true).to_string();
            let size = human_size(pack.installable_bytes(true));
            println!(
                "{}",
                tf(
                    "cli.modpack.files",
                    &[("files", files.as_str()), ("size", size.as_str())],
                )
            );
            Ok(())
        }
    }
}

async fn run_modpack_install(args: ModpackInstallArgs) -> Result<(), CliError> {
    let config = LauncherConfig::load()?;
    let minecraft_dir = resolve_minecraft_dir(args.minecraft_dir, &config)?;
    let java = args
        .java
        .or_else(|| configured_java(&config.java_path))
        .or_else(|| get_java_executable().map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("java"));
    let options = InstallOptions {
        minecraft_dir: &minecraft_dir,
        java: &java,
        with_optional: args.with_optional,
    };
    println!("{}", t("modpack.installing"));
    let reporter = Arc::new(|progress: PackProgress| {
        let line = progress.describe();
        if line.is_empty() {
            return;
        }
        print!("\r{line}");
        let _ = std::io::Write::flush(&mut std::io::stdout());
    });
    let report = modpack::install_file(&args.file, &options, reporter)
        .await
        .map_err(|error| CliError::Task(error.to_string()));
    println!();
    let report = report?;
    println!(
        "{}",
        tf("modpack.installed", &[("version", report.version_id.as_str())])
    );
    println!("{}", report.timings.summary());
    let files = report.files.to_string();
    let overrides = report.overrides.to_string();
    println!(
        "{}",
        tf(
            "modpack.installed_detail",
            &[("files", files.as_str()), ("overrides", overrides.as_str())]
        )
    );
    if !report.excluded.is_empty() {
        let skipped = report.excluded.join(", ");
        println!(
            "{}",
            tf("modpack.skipped_optional", &[("files", skipped.as_str())])
        );
    }
    let game_dir = report.game_dir.display().to_string();
    println!("{}", tf("common.saved_to", &[("path", game_dir.as_str())]));
    Ok(())
}

fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

fn run_config(action: ConfigAction) -> Result<(), CliError> {
    let mut config = LauncherConfig::load()?;
    match action {
        ConfigAction::Show => {
            let path = config::path()?;
            let path = path.display().to_string();
            println!("{}", tf("cli.config.file", &[("path", path.as_str())]));
            let dir = if config.minecraft_dir.trim().is_empty() {
                t("cli.config.default_dir")
            } else {
                config.minecraft_dir.trim()
            };
            let java = if config.java_path.trim().is_empty() {
                t("settings.java.auto")
            } else {
                config.java_path.trim()
            };
            println!("{}", tf("cli.config.minecraft_dir", &[("path", dir)]));
            println!("{}", tf("cli.config.java_path", &[("path", java)]));
            if !config.java_versions.is_empty() {
                println!("{}", t("cli.java.known"));
                for path in &config.java_versions {
                    println!("  {path}");
                }
            }
            println!("{}", tf("common.version_isolation", &[("value", yes_no(config.isolate))]));
            if config.instances.is_empty() {
                println!("{}", t("cli.config.overrides_none"));
            } else {
                println!("{}", t("cli.config.overrides"));
                for (id, entry) in &config.instances {
                    if entry.use_global {
                        println!(
                            "{}",
                            tf("cli.config.instance_global", &[("instance", id.as_str())])
                        );
                    } else {
                        let java = if entry.java_path.trim().is_empty() {
                            t("common.global_paren").to_owned()
                        } else {
                            entry.java_path.trim().to_owned()
                        };
                        println!(
                            "{}",
                            tf(
                                "cli.config.instance_detail",
                                &[
                                    ("instance", id.as_str()),
                                    ("isolate", yes_no(entry.isolate)),
                                    ("java", java.as_str()),
                                ],
                            )
                        );
                    }
                }
            }
        }
        ConfigAction::Set {
            minecraft_dir,
            java,
            isolate,
            no_isolate,
        } => {
            if let Some(dir) = minecraft_dir {
                config.minecraft_dir = dir.display().to_string();
            }
            if let Some(java) = java {
                config.java_path = java.display().to_string();
            }
            if isolate {
                config.isolate = true;
            } else if no_isolate {
                config.isolate = false;
            }
            let path = config.save()?;
            let path = path.display().to_string();
            println!("{}", tf("common.config_saved", &[("path", path.as_str())]));
        }
        ConfigAction::Instance {
            version,
            global,
            isolate,
            no_isolate,
            java,
        } => {
            if global {
                config.instances.insert(
                    version.clone(),
                    InstanceConfig {
                        use_global: true,
                        ..Default::default()
                    },
                );
            } else {
                if !isolate && !no_isolate && java.is_none() {
                    return Err(CliError::Usage(
                        t("cli.error.instance_flags")
                            .to_owned(),
                    ));
                }
                let entry = config.instances.entry(version.clone()).or_default();
                entry.use_global = false;
                if isolate {
                    entry.isolate = true;
                } else if no_isolate {
                    entry.isolate = false;
                }
                if let Some(java) = java {
                    entry.java_path = java.display().to_string();
                }
            }
            let path = config.save()?;
            let path = path.display().to_string();
            println!(
                "{}",
                tf(
                    "cli.config.instance_saved",
                    &[("instance", version.as_str()), ("path", path.as_str())],
                )
            );
        }
    }
    Ok(())
}

fn configured_java(value: &str) -> Option<PathBuf> {
    let value = value.trim();
    (!value.is_empty()).then(|| PathBuf::from(value))
}

fn yes_no(value: bool) -> &'static str {
    if value { t("common.on") } else { t("common.off") }
}

fn resolve_minecraft_dir(
    dir: Option<PathBuf>,
    config: &LauncherConfig,
) -> Result<PathBuf, AuthError> {
    if let Some(dir) = dir {
        return Ok(dir);
    }
    let configured = config.minecraft_dir.trim();
    if !configured.is_empty() {
        return Ok(PathBuf::from(configured));
    }
    Ok(std::env::current_dir()?.join(DEFAULT_MINECRAFT_DIR))
}

fn print_accounts(auth: &Auth) -> Result<(), AuthError> {
    let accounts = auth.list()?;
    if accounts.is_empty() {
        println!("{}", t("cli.account.none"));
        return Ok(());
    }
    let active = auth.active_uuid()?;
    for account in accounts {
        let marker = if active.as_deref() == Some(account.profile.id.as_str()) {
            "*"
        } else {
            " "
        };
        println!("{}", account_line(marker, &account));
    }
    Ok(())
}

fn account_line(marker: &str, account: &AccountSession) -> String {
    let state = match account.state {
        SessionState::Valid => t("account.state.valid"),
        SessionState::NeedsRelogin => t("account.state.relogin"),
        SessionState::Offline => t("account.state.offline"),
    };
    format!(
        "{marker} {}  {}  {state}",
        account.profile.name, account.profile.id
    )
}
