use std::io::Write;
use std::path::Path;
use std::time::Duration;

use mc_launcher_core::install::client::version_json_path;
use mc_launcher_core::install::{InstallRequest, InstallResult};
use mc_launcher_core::launcher::Launcher;
use mc_launcher_core::progress::ProgressEvent;
use serde::Deserialize;

const VERSION_MANIFEST_URL: &str =
    "https://launchermeta.mojang.com/mc/game/version_manifest_v2.json";
const REQUEST_TIMEOUT_SECONDS: u64 = 30;

/// 远端版本清单里的一条记录。
#[derive(Clone, Debug)]
pub struct AvailableVersion {
    pub id: String,
    pub kind: String,
}

#[derive(Deserialize)]
struct VersionManifest {
    versions: Vec<ManifestEntry>,
}

#[derive(Deserialize)]
struct ManifestEntry {
    id: String,
    #[serde(rename = "type")]
    kind: String,
}

/// 从 Mojang 拉取版本清单，顺序为新版本在前。
pub async fn available_versions() -> Result<Vec<AvailableVersion>, reqwest::Error> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(REQUEST_TIMEOUT_SECONDS))
        .build()?;
    let manifest: VersionManifest = client
        .get(VERSION_MANIFEST_URL)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    Ok(manifest
        .versions
        .into_iter()
        .map(|entry| AvailableVersion {
            id: entry.id,
            kind: entry.kind,
        })
        .collect())
}

/// 列出本地 `versions` 目录下已安装的实例（存在同名版本 JSON 才算）。
pub fn installed_versions(minecraft_dir: &Path) -> std::io::Result<Vec<String>> {
    let versions_dir = minecraft_dir.join("versions");
    if !versions_dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut versions = Vec::new();
    for entry in std::fs::read_dir(versions_dir)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let file_name = entry.file_name();
        let Some(name) = file_name.to_str() else {
            continue;
        };
        if version_json_path(minecraft_dir, name).is_file() {
            versions.push(name.to_owned());
        }
    }
    versions.sort();
    Ok(versions)
}

/// 安装过程中给调用方的精简进度。
#[derive(Clone, Debug)]
pub struct InstallProgress {
    pub downloaded: u64,
    pub skipped: u64,
    pub current: Option<String>,
}

pub fn install(
    minecraft_dir: &Path,
    version_id: &str,
) -> Result<InstallResult, mc_launcher_core::LauncherError> {
    let mut downloaded = 0_u64;
    let mut skipped = 0_u64;
    let mut shown = false;
    let result = run_install(minecraft_dir, version_id, |event: ProgressEvent| {
        match event {
            ProgressEvent::TaskFinished { .. } => downloaded += 1,
            ProgressEvent::TaskSkipped { .. } => skipped += 1,
            _ => return,
        }
        shown = true;
        print!("\rdownloaded {downloaded} files, skipped {skipped}");
        let _ = std::io::stdout().flush();
    });
    if shown {
        println!();
    }
    result
}

/// 与 [`install`] 相同，但把进度回调出去，供图形界面实时展示。
pub fn install_with_reporter(
    minecraft_dir: &Path,
    version_id: &str,
    reporter: &mut dyn FnMut(InstallProgress),
) -> Result<InstallResult, mc_launcher_core::LauncherError> {
    let mut downloaded = 0_u64;
    let mut skipped = 0_u64;
    let mut current: Option<String> = None;
    run_install(minecraft_dir, version_id, |event: ProgressEvent| {
        match event {
            ProgressEvent::TaskStarted { label, .. } => current = Some(label),
            ProgressEvent::TaskFinished { .. } => {
                downloaded += 1;
                current = None;
            }
            ProgressEvent::TaskSkipped { .. } => skipped += 1,
            _ => return,
        }
        reporter(InstallProgress {
            downloaded,
            skipped,
            current: current.clone(),
        });
    })
}

fn run_install<F>(
    minecraft_dir: &Path,
    version_id: &str,
    mut on_event: F,
) -> Result<InstallResult, mc_launcher_core::LauncherError>
where
    F: FnMut(ProgressEvent),
{
    let launcher = Launcher::new(minecraft_dir);
    launcher.install_with_progress(InstallRequest::vanilla(version_id), &mut on_event)
}
