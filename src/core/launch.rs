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
    #[error("Java check failed: {0}")]
    Java(String),
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
    if let Some(java) = request.java {
        ensure_java_available(java)?;
    }
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
    let mut child = Command::new(&command.executable);
    child
        .args(&command.args)
        .current_dir(&command.working_dir)
        .envs(command.env);
    apply_display_environment(&mut child);
    child.spawn().map_err(LaunchError::Spawn)
}

/// SDL 在 Wayland 会话里可能因为合成器缺少 fifo-v1 而退回 XWayland，
/// 而部分环境下 XWayland 创建不了 OpenGL 上下文（Minecraft 26.x 的 SDL 后端）。
/// 用户没有显式指定 SDL_VIDEO_DRIVER 时，优先使用原生 Wayland。
fn apply_display_environment(command: &mut Command) {
    let on_wayland = std::env::var_os("WAYLAND_DISPLAY").is_some_and(|value| !value.is_empty());
    let driver_set = std::env::var_os("SDL_VIDEO_DRIVER").is_some_and(|value| !value.is_empty());
    if on_wayland && !driver_set {
        command.env("SDL_VIDEO_DRIVER", "wayland");
    }
}

fn ensure_java_available(java: &Path) -> Result<(), LaunchError> {
    let output = Command::new(java).arg("-version").output().map_err(|error| {
        LaunchError::Java(format!(
            "cannot run Java executable `{}`: {error}",
            java.display()
        ))
    })?;
    if output.status.success() {
        Ok(())
    } else {
        Err(LaunchError::Java(format!(
            "`{} -version` exited with {}",
            java.display(),
            output.status
        )))
    }
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
