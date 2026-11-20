use std::path::Path;
use std::process::{Child, Command};

use mc_launcher_core::account::Account;
use mc_launcher_core::command::builder::LaunchOptions;
use mc_launcher_core::launcher::Launcher;
use thiserror::Error;

use super::auth::{GameToken, Profile};

const LAUNCHER_NAME: &str = "LYYSML";

#[derive(Debug, Error)]
pub enum LaunchError {
    #[error(transparent)]
    Core(#[from] mc_launcher_core::LauncherError),
    #[error("cannot start the game process: {0}")]
    Spawn(#[source] std::io::Error),
}

pub enum LaunchAccount<'a> {
    Microsoft(&'a GameToken),
    Offline(&'a Profile),
}

pub struct LaunchRequest<'a> {
    pub minecraft_dir: &'a Path,
    pub version_id: &'a str,
    pub account: LaunchAccount<'a>,
    pub java: Option<&'a Path>,
    pub game_dir: Option<&'a Path>,
}

pub fn launch(request: LaunchRequest<'_>) -> Result<Child, LaunchError> {
    let launcher = Launcher::new(request.minecraft_dir);
    let version = launcher.load_version(request.version_id)?;
    let options = LaunchOptions {
        account: core_account(&request.account),
        java_executable: request.java.map(Path::to_path_buf),
        // 原版启动器把游戏目录固定在 .minecraft 根目录，存档与资源跨版本共享
        game_directory: Some(
            request
                .game_dir
                .unwrap_or(request.minecraft_dir)
                .to_path_buf(),
        ),
        launcher_name: LAUNCHER_NAME.to_owned(),
        launcher_version: env!("CARGO_PKG_VERSION").to_owned(),
        ..Default::default()
    };
    let command = launcher.build_launch_command_from_version(&version, options)?;
    Command::new(&command.executable)
        .args(&command.args)
        .current_dir(&command.working_dir)
        .envs(command.env)
        .spawn()
        .map_err(LaunchError::Spawn)
}

fn core_account(account: &LaunchAccount<'_>) -> Account {
    match account {
        LaunchAccount::Microsoft(token) => Account::Microsoft {
            username: token.profile.name.clone(),
            uuid: token.profile.id.clone(),
            access_token: token.access_token.clone(),
        },
        // 离线身份必须沿用本地算出的 UUID，随机 UUID 会让联机存档对不上
        LaunchAccount::Offline(profile) => Account::Offline {
            username: profile.name.clone(),
            uuid: profile.id.clone(),
        },
    }
}
