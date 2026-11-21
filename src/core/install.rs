use std::io::Write;
use std::path::Path;

use mc_launcher_core::install::{InstallRequest, InstallResult};
use mc_launcher_core::launcher::Launcher;
use mc_launcher_core::progress::ProgressEvent;

pub fn install(
    minecraft_dir: &Path,
    version_id: &str,
) -> Result<InstallResult, mc_launcher_core::LauncherError> {
    let launcher = Launcher::new(minecraft_dir);
    let mut downloaded = 0_u64;
    let mut skipped = 0_u64;
    let mut shown = false;
    let mut reporter = |event: ProgressEvent| {
        match event {
            ProgressEvent::TaskFinished { .. } => downloaded += 1,
            ProgressEvent::TaskSkipped { .. } => skipped += 1,
            _ => return,
        }
        shown = true;
        print!("\rdownloaded {downloaded} files, skipped {skipped}");
        let _ = std::io::stdout().flush();
    };
    let result = launcher.install_with_progress(InstallRequest::vanilla(version_id), &mut reporter);
    if shown {
        println!();
    }
    result
}
