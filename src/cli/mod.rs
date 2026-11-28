use std::path::{Path, PathBuf};
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
use crate::core::logging;
use crate::core::launch::{self, LaunchAccount, LaunchError, LaunchRequest};
use crate::core::modpack::{self, InstallOptions, PackProgress};
use crate::core::modrinth::{self, ModrinthError, ModSearchRequest};
use crate::gui;

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
    /// `detail` is the English text written to file logs; `key` and `args`
    /// render the translated terminal message separately.
    #[error("{detail}")]
    Usage {
        key: &'static str,
        args: Vec<(String, String)>,
        detail: String,
    },
    #[error("gui failed: {0}")]
    Gui(String),
}

/// Build a usage error. `key`/`args` select the translated terminal text;
/// the English rendering of the same key is kept for file logs.
fn usage(key: &'static str, args: &[(&str, &str)]) -> CliError {
    CliError::Usage {
        key,
        args: args
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect(),
        detail: i18n::tf_en(key, args),
    }
}

impl CliError {
    /// Stable identifier written to file logs across versions and locales.
    pub fn code(&self) -> &'static str {
        match self {
            Self::Auth(_) => "auth",
            Self::Launch(_) => "launch",
            Self::Modrinth(_) => "modrinth",
            Self::Config(_) => "config",
            Self::Core(_) => "core",
            Self::Game(_) => "game_exit",
            Self::Task(_) => "install_task",
            Self::UnknownLoader(_) => "unknown_loader",
            Self::Usage { .. } => "usage",
            Self::Gui(_) => "gui",
        }
    }

    /// English diagnostic for file logs, formatted from the structured
    /// error and never passed through the i18n system.
    pub fn log_detail(&self) -> String {
        match self {
            Self::Usage { detail, .. } => detail.clone(),
            other => other.to_string(),
        }
    }

    /// Translated message for the terminal.
    pub fn user_message(&self) -> String {
        match self {
            Self::Auth(error) => error.message(),
            Self::Launch(error) => error.message(),
            Self::Modrinth(error) => error.message(),
            Self::Config(error) => {
                let detail = error.to_string();
                tf("cli.error.config", &[("error", detail.as_str())])
            }
            Self::Core(error) => {
                let detail = error.to_string();
                tf("cli.error.core", &[("error", detail.as_str())])
            }
            Self::Game(status) => {
                let status = status.to_string();
                tf("cli.error.game_exit", &[("status", status.as_str())])
            }
            Self::Task(detail) => tf("cli.error.task_failed", &[("error", detail.as_str())]),
            Self::UnknownLoader(loader) => {
                tf("cli.error.unknown_loader", &[("loader", loader.as_str())])
            }
            Self::Gui(detail) => tf("cli.error.gui_failed", &[("error", detail.as_str())]),
            Self::Usage { key, args, .. } => {
                let args = args
                    .iter()
                    .map(|(name, value)| (name.as_str(), value.as_str()))
                    .collect::<Vec<_>>();
                tf(key, &args)
            }
        }
    }
}

#[derive(Parser)]
#[command(version, about = "LYYS MC Launcher")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Manage Microsoft and offline accounts.
    Account(AccountArgs),
    /// Install a Minecraft version, optionally with a mod loader.
    Install(InstallArgs),
    /// Launch an installed version and wait for the game to exit.
    Launch(LaunchArgs),
    /// List the versions installed in the active Minecraft directory.
    List,
    /// List the Java runtimes found on this machine.
    Java,
    /// Search projects on Modrinth.
    Search(SearchArgs),
    /// Download a Modrinth project file into an instance.
    Download(DownloadArgs),
    /// Inspect or install a local modpack archive.
    Modpack(ModpackArgs),
    /// Show or change launcher configuration.
    Config(ConfigArgs),
    /// Start the graphical interface (default when no command is given).
    Gui,
}

#[derive(Args)]
struct AccountArgs {
    #[command(subcommand)]
    action: AccountAction,
}

#[derive(Subcommand)]
enum AccountAction {
    /// Sign in with a Microsoft account (device code flow).
    Login,
    /// Add an offline account by name.
    Offline { name: String },
    /// List all saved accounts, marking the active one.
    List,
    /// Select the account used for launching.
    Use {
        /// Account name or UUID.
        account: String,
    },
    /// Remove a saved account.
    Remove {
        /// Account name or UUID.
        account: String,
    },
    /// Show the currently selected account.
    Current,
}

#[derive(Args)]
struct InstallArgs {
    /// Minecraft version to install, e.g. 1.20.1.
    version: String,
    /// Minecraft directory to install into (defaults to the active one).
    #[arg(long)]
    minecraft_dir: Option<PathBuf>,
    /// Mod loader to install: fabric, quilt, forge or neoforge.
    #[arg(long)]
    loader: Option<String>,
    /// Exact loader version; defaults to the latest compatible one.
    #[arg(long, requires = "loader")]
    loader_version: Option<String>,
    /// Display name recorded for the installed instance.
    #[arg(long)]
    name: Option<String>,
    /// Java executable used by the Forge/NeoForge installer (defaults to the configured one).
    #[arg(long)]
    java: Option<PathBuf>,
}

#[derive(Args)]
struct LaunchArgs {
    /// Installed version or instance to launch, e.g. 1.20.1.
    version: String,
    /// Minecraft directory containing the version (defaults to the active one).
    #[arg(long)]
    minecraft_dir: Option<PathBuf>,
    /// Game directory to run the version from.
    #[arg(long)]
    game_dir: Option<PathBuf>,
    /// Java executable to run the game with.
    #[arg(long)]
    java: Option<PathBuf>,
    /// Maximum Java heap in MiB for this launch; 0 uses the JVM default.
    #[arg(long)]
    memory: Option<u32>,
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
        /// Register the directory (if it is new) and make it the active one.
        #[arg(long)]
        minecraft_dir: Option<PathBuf>,
        /// Java executable used when no version-specific override exists.
        #[arg(long)]
        java: Option<PathBuf>,
        /// Enable version isolation.
        #[arg(long, conflicts_with = "no_isolate")]
        isolate: bool,
        /// Disable version isolation.
        #[arg(long)]
        no_isolate: bool,
        /// Maximum Java heap in MiB; 0 uses the JVM default.
        #[arg(long)]
        memory: Option<u32>,
    },
    /// Manage the Minecraft directories known to the launcher.
    Dir {
        /// Register a directory (if it is new) and make it active.
        #[arg(long)]
        add: Option<PathBuf>,
        /// Make an already registered directory the active one.
        #[arg(long = "use")]
        use_dir: Option<PathBuf>,
        /// Forget a registered directory.
        #[arg(long)]
        remove: Option<PathBuf>,
        /// Display name applied together with --add or --use.
        #[arg(long)]
        name: Option<String>,
    },
    /// Use the global configuration or override it for one instance.
    Instance {
        /// Version or instance id to configure.
        version: String,
        /// Fall back to the global configuration.
        #[arg(long, conflicts_with_all = ["isolate", "no_isolate", "java", "memory"])]
        global: bool,
        /// Enable version isolation for this instance.
        #[arg(long, conflicts_with = "no_isolate")]
        isolate: bool,
        /// Disable version isolation for this instance.
        #[arg(long)]
        no_isolate: bool,
        /// Java executable for this instance.
        #[arg(long)]
        java: Option<PathBuf>,
        /// Maximum Java heap in MiB for this instance; 0 inherits the global value.
        #[arg(long)]
        memory: Option<u32>,
    },
}

#[derive(Args)]
struct SearchArgs {
    /// Search query; leave empty to list popular projects.
    #[arg(long)]
    query: Option<String>,
    /// Project type: mod, modpack, resourcepack, shader or datapack.
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
    /// Project type: mod, resourcepack, shader or datapack (modpacks use `modpack install`).
    #[arg(long = "type", value_enum, default_value = "mod")]
    kind: ContentType,
    /// Minecraft version used to pick a compatible file.
    #[arg(long)]
    minecraft: String,
    /// Mod loader; required when installing mods.
    #[arg(long)]
    loader: Option<String>,
    /// Minecraft directory that owns the target instance (defaults to the active one).
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
    Info {
        /// Path to the modpack archive.
        file: PathBuf,
    },
}

#[derive(Args)]
struct ModpackInstallArgs {
    /// Path to the modpack archive.
    file: PathBuf,
    /// Minecraft directory to install into (defaults to the active one).
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
    Modpack,
    Resourcepack,
    Shader,
    Datapack,
}

impl From<ContentType> for modrinth::ProjectType {
    fn from(kind: ContentType) -> Self {
        match kind {
            ContentType::Mod => Self::Mod,
            ContentType::Modpack => Self::Modpack,
            ContentType::Resourcepack => Self::ResourcePack,
            ContentType::Shader => Self::ShaderPack,
            ContentType::Datapack => Self::DataPack,
        }
    }
}

pub async fn run() -> Result<(), CliError> {
    logging::init();
    let language = LauncherConfig::load()
        .ok()
        .and_then(|config| i18n::Language::from_code(&config.language))
        .unwrap_or_else(i18n::Language::detect);
    i18n::set_language(language);
    let command = Cli::parse().command;
    logging::info(&format!(
        "launcher {} started, command {}, terminal language {}",
        env!("CARGO_PKG_VERSION"),
        command_name(&command),
        language.code()
    ));
    let result = match command {
        Some(Command::Account(args)) => run_account(args.action).await,
        Some(Command::Install(args)) => run_install(args).await,
        Some(Command::Launch(args)) => run_launch(args).await,
        Some(Command::List) => run_list(),
        Some(Command::Java) => run_java_list(),
        Some(Command::Search(args)) => run_search(args).await,
        Some(Command::Download(args)) => run_download(args).await,
        Some(Command::Modpack(args)) => run_modpack(args).await,
        Some(Command::Config(args)) => run_config(args.action),
        Some(Command::Gui) | None => gui::run().map_err(CliError::Gui),
    };
    match &result {
        Ok(()) => logging::info("command finished successfully"),
        Err(error) => logging::error(error.code(), &error.log_detail()),
    }
    result
}

fn command_name(command: &Option<Command>) -> &'static str {
    match command {
        Some(Command::Account(_)) => "account",
        Some(Command::Install(_)) => "install",
        Some(Command::Launch(_)) => "launch",
        Some(Command::List) => "list",
        Some(Command::Java) => "java",
        Some(Command::Search(_)) => "search",
        Some(Command::Download(_)) => "download",
        Some(Command::Modpack(_)) => "modpack",
        Some(Command::Config(_)) => "config",
        Some(Command::Gui) | None => "gui",
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
            let session = auth.add_offline(&name).await?;
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
        AccountAction::Use { account } => {
            let resolved = resolve_account(&auth, &account)?;
            auth.set_active(&resolved.profile.id)?;
            println!(
                "{}",
                tf("cli.account.active_set", &[("account", resolved.profile.name.as_str())])
            );
        }
        AccountAction::Remove { account } => {
            let resolved = resolve_account(&auth, &account)?;
            auth.remove(&resolved.profile.id)?;
            println!(
                "{}",
                tf("cli.account.removed", &[("account", resolved.profile.name.as_str())])
            );
        }
        AccountAction::Current => {
            let session = auth.refresh_current().await?;
            println!("{}", account_line("*", &session));
        }
    }
    Ok(())
}

/// 按 UUID 或玩家名解析账号；名称先精确匹配再忽略大小写。
fn resolve_account(auth: &Auth, query: &str) -> Result<AccountSession, CliError> {
    let query = query.trim();
    let accounts = auth.list()?;
    let matched = accounts
        .iter()
        .find(|account| account.profile.id.eq_ignore_ascii_case(query))
        .or_else(|| accounts.iter().find(|account| account.profile.name == query))
        .or_else(|| {
            accounts
                .iter()
                .find(|account| account.profile.name.eq_ignore_ascii_case(query))
        });
    match matched {
        Some(account) => Ok(account.clone()),
        None => Err(usage("cli.error.account_not_found", &[("account", query)])),
    }
}

async fn run_install(args: InstallArgs) -> Result<(), CliError> {
    let mut config = LauncherConfig::load()?;
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
    let install_mirror = config.download_mirror.trim().to_owned();
    let result = tokio::task::spawn_blocking(move || match loader {
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
                &install_mirror,
                &mut reporter,
            );
            if printed {
                println!();
            }
            result
        }
        None => install::install(&install_dir, &install_version, &install_mirror),
    })
    .await
    .map_err(|error| CliError::Task(error.to_string()))??;
    if let Some(name) = args
        .name
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty())
    {
        // 显式指定的目录可能尚未登记，先登记再写名字，否则命名没有归属
        config.register_dir(&minecraft_dir.display().to_string());
        config.set_instance_name(&minecraft_dir, &result.version_id, name);
        config.save()?;
    }
    logging::info(&format!(
        "installed version {version_id} into {}",
        minecraft_dir.display()
    ));
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
    validate_memory(args.memory)?;
    let config = LauncherConfig::load()?;
    let minecraft_dir = resolve_minecraft_dir(args.minecraft_dir, &config)?;
    let installed = install::installed_versions(&minecraft_dir)
        .map_err(|error| mc_launcher_core::LauncherError::Io { source: error })?;
    if !installed.iter().any(|id| id == &args.version) {
        return Err(usage(
            "cli.error.version_not_installed",
            &[("version", args.version.as_str())],
        ));
    }
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
    let memory_mb = args
        .memory
        .unwrap_or_else(|| config.effective_memory(&args.version));

    let auth = Auth::new()?;
    let session = auth.refresh_current().await?;
    if session.state == SessionState::Offline {
        auth.ensure_offline_allowed().await?;
    }
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
        memory_mb,
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
    logging::info(&format!("launched version {}, pid {}", args.version, child.id()));
    // 子进程继承终端输出，不等到它退出就交还 shell 会把游戏日志和提示符搅在一起
    let status = child.wait().map_err(AuthError::from)?;
    if !status.success() {
        return Err(CliError::Game(status));
    }
    Ok(())
}

/// Print the versions installed in the active Minecraft directory.
fn run_list() -> Result<(), CliError> {
    let config = LauncherConfig::load()?;
    let minecraft_dir = resolve_minecraft_dir(None, &config)?;
    let dir = minecraft_dir.display().to_string();
    let versions = install::installed_versions(&minecraft_dir)
        .map_err(|error| mc_launcher_core::LauncherError::Io { source: error })?;
    if versions.is_empty() {
        println!("{}", tf("cli.list.none", &[("dir", dir.as_str())]));
        return Ok(());
    }
    println!("{}", tf("cli.list.header", &[("dir", dir.as_str())]));
    for version in &versions {
        let name = config.instance_name(version);
        if name.is_empty() {
            println!("  {version}");
        } else {
            println!("  {name}  ({version})");
        }
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
        let downloads = group_thousands(hit.downloads);
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
    let total_hits = group_thousands(result.total_hits);
    let shown = result.hits.len().to_string();
    println!(
        "{}",
        tf("cli.search.results", &[("total", total_hits.as_str()), ("shown", shown.as_str())])
    );
    Ok(())
}

async fn run_download(args: DownloadArgs) -> Result<(), CliError> {
    let kind = modrinth::ProjectType::from(args.kind);
    if kind == modrinth::ProjectType::Modpack {
        return Err(usage("cli.error.modpack_command", &[]));
    }
    let game_version = args.minecraft.trim();
    if game_version.is_empty() {
        return Err(usage("cli.error.minecraft_empty", &[]));
    }
    let loader = args.loader.as_deref().map(str::trim).unwrap_or("");
    if kind == modrinth::ProjectType::Mod && loader.is_empty() {
        return Err(usage("cli.error.mods_loader", &[]));
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
                        return Err(usage("cli.error.datapack_world", &[]));
                    }
                    game_dir.join("saves").join(world).join("datapacks")
                }
                modrinth::ProjectType::Modpack => {
                    return Err(usage("cli.error.modpack_command", &[]));
                }
            }
        }
    };

    let outcome = match kind {
        modrinth::ProjectType::Mod => {
            modrinth::install_with_dependencies(&args.project, game_version, loader, &target, &mut |_| {})
                .await?
        }
        _ => modrinth::install_content(&args.project, game_version, &target, &mut |_| {}).await?,
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
    logging::info(&format!(
        "downloaded {} file(s) into {}",
        outcome.installed.len(),
        target.display()
    ));
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
            if !file.exists() {
                let path = file.display().to_string();
                return Err(usage(
                    "cli.error.modpack_file_not_found",
                    &[("path", path.as_str())],
                ));
            }
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
    if !args.file.exists() {
        let path = args.file.display().to_string();
        return Err(usage(
            "cli.error.modpack_file_not_found",
            &[("path", path.as_str())],
        ));
    }
    logging::info(&format!("installing modpack {}", args.file.display()));
    let mut config = LauncherConfig::load()?;
    let minecraft_dir = resolve_minecraft_dir(args.minecraft_dir, &config)?;
    let java = args
        .java
        .or_else(|| configured_java(&config.java_path))
        .or_else(|| get_java_executable().map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from("java"));
    let mirror = config.download_mirror.trim().to_owned();
    let options = InstallOptions {
        minecraft_dir: &minecraft_dir,
        java: &java,
        mirror: &mirror,
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
    logging::info(&format!(
        "installed modpack as version {} in {}",
        report.version_id,
        report.game_dir.display()
    ));
    // 整合包放在隔离目录里，安装完成后把该版本标记为隔离实例；
    // 显式指定的安装目录先登记，否则隔离标记没有归属
    config.register_dir(&minecraft_dir.display().to_string());
    modpack::isolate_instance(&mut config, &minecraft_dir, &report.version_id);
    config.save()?;
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

fn group_thousands(value: u64) -> String {
    let digits = value.to_string();
    let mut output = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, character) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index) % 3 == 0 {
            output.push(',');
        }
        output.push(character);
    }
    output
}

fn validate_java_path(path: &Path) -> Result<(), CliError> {
    if !path.is_file() {
        let text = path.display().to_string();
        return Err(usage("cli.error.java_not_found", &[("path", text.as_str())]));
    }
    Ok(())
}

fn run_config(action: ConfigAction) -> Result<(), CliError> {
    let mut config = LauncherConfig::load()?;
    match action {
        ConfigAction::Show => {
            let path = config::path()?;
            let path = path.display().to_string();
            println!("{}", tf("cli.config.file", &[("path", path.as_str())]));
            let java = if config.java_path.trim().is_empty() {
                t("settings.java.auto")
            } else {
                config.java_path.trim()
            };
            println!("{}", tf("cli.config.java_path", &[("path", java)]));
            let memory = memory_label(config.memory_mb);
            println!(
                "{}",
                tf("cli.config.memory", &[("memory", memory.as_str())])
            );
            if !config.java_versions.is_empty() {
                println!("{}", t("cli.java.known"));
                for path in &config.java_versions {
                    println!("  {path}");
                }
            }
            println!("{}", tf("common.version_isolation", &[("value", yes_no(config.isolate))]));
            print_dirs(&config);
        }
        ConfigAction::Set {
            minecraft_dir,
            java,
            isolate,
            no_isolate,
            memory,
        } => {
            validate_memory(memory)?;
            if let Some(dir) = minecraft_dir {
                config.select_dir(&dir.display().to_string());
            }
            if let Some(java) = java {
                validate_java_path(&java)?;
                config.java_path = java.display().to_string();
            }
            if isolate {
                config.isolate = true;
            } else if no_isolate {
                config.isolate = false;
            }
            if let Some(memory) = memory {
                config.memory_mb = memory;
            }
            let path = config.save()?;
            let path = path.display().to_string();
            println!("{}", tf("common.config_saved", &[("path", path.as_str())]));
        }
        ConfigAction::Dir {
            add,
            use_dir,
            remove,
            name,
        } => {
            let selected = [add.is_some(), use_dir.is_some(), remove.is_some()]
                .iter()
                .filter(|chosen| **chosen)
                .count();
            if selected > 1 {
                return Err(usage("cli.dir.one_action", &[]));
            }
            if name.is_some() && add.is_none() && use_dir.is_none() {
                return Err(usage("cli.dir.name_requires_target", &[]));
            }
            if let Some(path) = add {
                if !path.is_dir() {
                    let text = path.display().to_string();
                    return Err(usage(
                        "cli.error.directory_not_found",
                        &[("path", text.as_str())],
                    ));
                }
                let path = path.display().to_string();
                config.select_dir(&path);
                if let Some(name) = name {
                    if let Some(dir) = config.active_dir_mut() {
                        dir.name = name;
                    }
                }
            } else if let Some(path) = use_dir {
                let path = path.display().to_string();
                let index = config.dir_index(&path).ok_or_else(|| {
                    usage("cli.dir.not_found", &[("path", path.as_str())])
                })?;
                config.active_minecraft_dir = config.minecraft_dirs[index].path.clone();
                if let Some(name) = name {
                    config.minecraft_dirs[index].name = name;
                }
            } else if let Some(path) = remove {
                let path = path.display().to_string();
                let index = config.dir_index(&path).ok_or_else(|| {
                    usage("cli.dir.not_found", &[("path", path.as_str())])
                })?;
                if config.minecraft_dirs.len() <= 1 {
                    return Err(usage("cli.dir.keep_one", &[]));
                }
                config.minecraft_dirs.remove(index);
            } else {
                print_dirs(&config);
                return Ok(());
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
            memory,
        } => {
            // 实例覆盖只作用于当前目录，避免不同目录下的同名版本互相影响
            validate_memory(memory)?;
            if config.minecraft_dirs.is_empty() {
                config.select_dir(config::DEFAULT_MINECRAFT_DIR);
            }
            let dir = config
                .active_dir_mut()
                .expect("a minecraft directory exists after select_dir");
            if global {
                dir.instances.insert(
                    version.clone(),
                    InstanceConfig {
                        use_global: true,
                        ..Default::default()
                    },
                );
            } else {
                if !isolate && !no_isolate && java.is_none() && memory.is_none() {
                    return Err(usage("cli.error.instance_flags", &[]));
                }
                let entry = dir.instances.entry(version.clone()).or_default();
                entry.use_global = false;
                if isolate {
                    entry.isolate = true;
                } else if no_isolate {
                    entry.isolate = false;
                }
                if let Some(java) = java {
                    validate_java_path(&java)?;
                    entry.java_path = java.display().to_string();
                }
                if let Some(memory) = memory {
                    entry.memory_mb = memory;
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

fn validate_memory(memory: Option<u32>) -> Result<(), CliError> {
    let Some(memory) = memory.filter(|value| *value > config::MAX_MEMORY_MB) else {
        return Ok(());
    };
    let value = memory.to_string();
    let max = config::MAX_MEMORY_MB.to_string();
    Err(usage(
        "cli.error.memory_range",
        &[("value", value.as_str()), ("max", max.as_str())],
    ))
}

fn memory_label(memory_mb: u32) -> String {
    if memory_mb == 0 {
        t("settings.memory_auto").to_owned()
    } else {
        format!("{memory_mb} MB")
    }
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
    let configured = config
        .active_dir()
        .map(|dir| dir.path.trim())
        .unwrap_or("");
    if !configured.is_empty() {
        return Ok(PathBuf::from(configured));
    }
    Ok(std::env::current_dir()?.join(config::DEFAULT_MINECRAFT_DIR))
}

fn print_dirs(config: &LauncherConfig) {
    if config.minecraft_dirs.is_empty() {
        println!("{}", t("cli.config.dirs_none"));
        return;
    }
    println!("{}", t("cli.config.dirs"));
    let active = config.active_index();
    for (index, dir) in config.minecraft_dirs.iter().enumerate() {
        let path = dir.path.trim();
        let name = dir.name.trim();
        let label = if name.is_empty() {
            path.to_owned()
        } else {
            format!("{name}  ({path})")
        };
        let marker = if active == Some(index) {
            t("cli.config.dir_active")
        } else {
            ""
        };
        println!(
            "{}",
            tf(
                "cli.config.dir_item",
                &[("label", label.as_str()), ("active", marker)],
            )
        );
        if dir.instances.is_empty() {
            continue;
        }
        println!("{}", t("cli.config.overrides"));
        for (id, entry) in &dir.instances {
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
                let memory = if entry.memory_mb == 0 {
                    t("common.global_paren").to_owned()
                } else {
                    memory_label(entry.memory_mb)
                };
                println!(
                    "{}",
                    tf(
                        "cli.config.instance_detail",
                        &[
                            ("instance", id.as_str()),
                            ("isolate", yes_no(entry.isolate)),
                            ("java", java.as_str()),
                            ("memory", memory.as_str()),
                        ],
                    )
                );
            }
        }
    }
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
