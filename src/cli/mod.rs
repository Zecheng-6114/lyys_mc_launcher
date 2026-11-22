use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};
use mc_launcher_core::loader::common::{LoaderSpec, LoaderVersion};
use mc_launcher_core::utils::get_java_executable;
use thiserror::Error;

use crate::core::auth::{AccountSession, Auth, AuthError, SessionState};
use crate::core::install;
use crate::core::launch::{self, LaunchAccount, LaunchError, LaunchRequest};
use crate::gui;

const DEFAULT_MINECRAFT_DIR: &str = ".minecraft";

#[derive(Debug, Error)]
pub enum CliError {
    #[error(transparent)]
    Auth(#[from] AuthError),
    #[error(transparent)]
    Launch(#[from] LaunchError),
    #[error(transparent)]
    Core(#[from] mc_launcher_core::LauncherError),
    #[error("the game exited with {0}")]
    Game(std::process::ExitStatus),
    #[error("install task failed: {0}")]
    Task(String),
    #[error("unknown loader {0}, expected fabric, quilt, forge or neoforge")]
    UnknownLoader(String),
    #[error("gui failed: {0}")]
    Gui(String),
}

#[derive(Parser)]
#[command(version, about = "LYYS Minecraft launcher")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    Account(AccountArgs),
    Install(InstallArgs),
    Launch(LaunchArgs),
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
    /// Java executable used by the Forge/NeoForge installer (defaults to the detected Java).
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
    #[arg(long, conflicts_with = "game_dir")]
    isolate: bool,
}

pub async fn run() -> Result<(), CliError> {
    match Cli::parse().command {
        Some(Command::Account(args)) => run_account(args.action).await,
        Some(Command::Install(args)) => run_install(args).await,
        Some(Command::Launch(args)) => run_launch(args).await,
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
                        "open {} in a browser and enter code {}",
                        prompt.verification_uri, prompt.user_code
                    );
                    println!("the code is valid for {} minutes", prompt.expires_in / 60);
                })
                .await?;
            println!(
                "logged in: {} ({})",
                session.profile.name, session.profile.id
            );
        }
        AccountAction::List => print_accounts(&auth)?,
        AccountAction::Offline { name } => {
            let session = auth.add_offline(&name)?;
            println!(
                "offline account added: {} ({})",
                session.profile.name, session.profile.id
            );
        }
        AccountAction::Use { uuid } => {
            auth.set_active(&uuid)?;
            println!("active account set to {uuid}");
        }
        AccountAction::Remove { uuid } => {
            auth.remove(&uuid)?;
            println!("account removed: {uuid}");
        }
        AccountAction::Current => {
            let session = auth.refresh_current().await?;
            println!("{}", account_line("*", &session));
        }
    }
    Ok(())
}

async fn run_install(args: InstallArgs) -> Result<(), CliError> {
    let minecraft_dir = resolve_minecraft_dir(args.minecraft_dir)?;
    let version_id = args.version;
    let java = args
        .java
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
                print!(
                    "\rdownloaded {} files, skipped {}",
                    progress.downloaded, progress.skipped
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
    println!("installed {} into {}", version_id, minecraft_dir.display());
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
    let minecraft_dir = resolve_minecraft_dir(args.minecraft_dir)?;

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
        java: args.java.as_deref(),
        game_dir: args.game_dir.as_deref(),
        isolated: args.isolate,
    })?;
    println!("launched {}, pid {}", args.version, child.id());
    // 子进程继承终端输出，不等到它退出就交还 shell 会把游戏日志和提示符搅在一起
    let status = child.wait().map_err(AuthError::from)?;
    if !status.success() {
        return Err(CliError::Game(status));
    }
    Ok(())
}

fn resolve_minecraft_dir(dir: Option<PathBuf>) -> Result<PathBuf, AuthError> {
    match dir {
        Some(dir) => Ok(dir),
        None => Ok(std::env::current_dir()?.join(DEFAULT_MINECRAFT_DIR)),
    }
}

fn print_accounts(auth: &Auth) -> Result<(), AuthError> {
    let accounts = auth.list()?;
    if accounts.is_empty() {
        println!("no accounts yet, run account login to add one");
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
        SessionState::Valid => "valid",
        SessionState::NeedsRelogin => "needs relogin",
        SessionState::Offline => "offline",
    };
    format!(
        "{marker} {}  {}  {state}",
        account.profile.name, account.profile.id
    )
}
