use std::path::{Path, PathBuf};
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
    #[error("cannot resolve a launch path: {0}")]
    Absolute(#[source] std::io::Error),
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
    pub isolated: bool,
}

pub fn launch(request: LaunchRequest<'_>) -> Result<Child, LaunchError> {
    // 类路径和 natives 路径都基于 Minecraft 目录；如果目录是相对路径，
    // 而工作目录又被切换到游戏目录，Java 会按新工作目录解析这些相对路径导致找不到主类。
    let minecraft_dir =
        std::path::absolute(request.minecraft_dir).map_err(LaunchError::Absolute)?;
    let launcher = Launcher::new(&minecraft_dir);
    let version = launcher.load_version(request.version_id)?;
    let options = LaunchOptions {
        account: core_account(&request.account),
        java_executable: request.java.map(Path::to_path_buf),
        game_directory: Some(game_directory(&minecraft_dir, &request)?),
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

fn game_directory(
    minecraft_dir: &Path,
    request: &LaunchRequest<'_>,
) -> Result<PathBuf, LaunchError> {
    if let Some(dir) = request.game_dir {
        return std::path::absolute(dir).map_err(LaunchError::Absolute);
    }
    if request.isolated {
        return Ok(minecraft_dir
            .join("versions")
            .join(request.version_id));
    }
    // 原版启动器把游戏目录固定在 .minecraft 根目录，存档与资源跨版本共享
    Ok(minecraft_dir.to_path_buf())
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
