use std::path::PathBuf;

use thiserror::Error;

use crate::core::auth::{AccountSession, Auth, AuthError, SessionState};
use crate::core::launch::{self, LaunchAccount, LaunchError, LaunchRequest};

const USAGE: &str = "usage:
  lyys_mc_launcher account login
  lyys_mc_launcher account offline <name>
  lyys_mc_launcher account list
  lyys_mc_launcher account use <uuid>
  lyys_mc_launcher account remove <uuid>
  lyys_mc_launcher account current
  lyys_mc_launcher launch <version> [--minecraft-dir <dir>] [--game-dir <dir>] [--java <path>] [--isolate]";

const ACCOUNT_COMMAND: &str = "account";
const LAUNCH_COMMAND: &str = "launch";
const DEFAULT_MINECRAFT_DIR: &str = ".minecraft";

#[derive(Debug, Error)]
pub enum CliError {
    #[error(transparent)]
    Auth(#[from] AuthError),
    #[error(transparent)]
    Launch(#[from] LaunchError),
    #[error("the game exited with {0}")]
    Game(std::process::ExitStatus),
}

pub async fn run() -> Result<(), CliError> {
    let mut args = std::env::args().skip(1);
    let command = args
        .next()
        .ok_or_else(|| AuthError::InvalidArgument(USAGE.to_owned()))?;
    match command.as_str() {
        ACCOUNT_COMMAND => run_account(&mut args).await,
        LAUNCH_COMMAND => run_launch(&mut args).await,
        other => {
            Err(AuthError::InvalidArgument(format!("unknown command {other}\n{USAGE}")).into())
        }
    }
}

async fn run_account(args: &mut impl Iterator<Item = String>) -> Result<(), CliError> {
    let action = args
        .next()
        .ok_or_else(|| AuthError::InvalidArgument(USAGE.to_owned()))?;
    let auth = Auth::new()?;
    match action.as_str() {
        "login" => {
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
        "list" => print_accounts(&auth)?,
        "offline" => {
            let name = take_name(args)?;
            let session = auth.add_offline(&name)?;
            println!(
                "offline account added: {} ({})",
                session.profile.name, session.profile.id
            );
        }
        "use" => {
            let uuid = take_uuid(args)?;
            auth.set_active(&uuid)?;
            println!("active account set to {uuid}");
        }
        "remove" => {
            let uuid = take_uuid(args)?;
            auth.remove(&uuid)?;
            println!("account removed: {uuid}");
        }
        "current" => {
            let session = auth.refresh_current().await?;
            println!("{}", account_line("*", &session));
        }
        other => {
            return Err(AuthError::InvalidArgument(format!(
                "unknown account command {other}\n{USAGE}"
            ))
            .into());
        }
    }
    Ok(())
}

async fn run_launch(args: &mut impl Iterator<Item = String>) -> Result<(), CliError> {
    let version_id = args
        .next()
        .ok_or_else(|| AuthError::InvalidArgument(format!("missing version\n{USAGE}")))?;
    let mut minecraft_dir = None;
    let mut game_dir = None;
    let mut java = None;
    let mut isolated = false;
    while let Some(option) = args.next() {
        match option.as_str() {
            "--minecraft-dir" => {
                minecraft_dir = Some(PathBuf::from(take_option_value(args, &option)?))
            }
            "--game-dir" => game_dir = Some(PathBuf::from(take_option_value(args, &option)?)),
            "--java" => java = Some(PathBuf::from(take_option_value(args, &option)?)),
            "--isolate" => isolated = true,
            other => {
                return Err(AuthError::InvalidArgument(format!(
                    "unknown launch option {other}\n{USAGE}"
                ))
                .into());
            }
        }
    }
    if isolated && game_dir.is_some() {
        return Err(AuthError::InvalidArgument(
            "--isolate cannot be combined with --game-dir".to_owned(),
        )
        .into());
    }
    let minecraft_dir = match minecraft_dir {
        Some(dir) => dir,
        None => std::env::current_dir()
            .map_err(AuthError::from)?
            .join(DEFAULT_MINECRAFT_DIR),
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
        version_id: &version_id,
        account,
        java: java.as_deref(),
        game_dir: game_dir.as_deref(),
        isolated,
    })?;
    println!("launched {version_id}, pid {}", child.id());
    // 子进程继承终端输出，不等到它退出就交还 shell 会把游戏日志和提示符搅在一起
    let status = child.wait().map_err(AuthError::from)?;
    if !status.success() {
        return Err(CliError::Game(status));
    }
    Ok(())
}

fn take_option_value(
    args: &mut impl Iterator<Item = String>,
    option: &str,
) -> Result<String, AuthError> {
    args.next()
        .ok_or_else(|| AuthError::InvalidArgument(format!("missing value for {option}")))
}

fn take_uuid(args: &mut impl Iterator<Item = String>) -> Result<String, AuthError> {
    take_value(args, "missing account uuid")
}

fn take_name(args: &mut impl Iterator<Item = String>) -> Result<String, AuthError> {
    take_value(args, "missing offline username")
}

fn take_value(args: &mut impl Iterator<Item = String>, missing: &str) -> Result<String, AuthError> {
    args.next()
        .ok_or_else(|| AuthError::InvalidArgument(missing.to_owned()))
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
