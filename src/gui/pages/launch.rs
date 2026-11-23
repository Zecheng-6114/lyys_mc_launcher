use std::path::{Path, PathBuf};
use std::sync::Arc;

use eframe::egui;

use crate::core::auth::{Auth, SessionState};
use crate::core::i18n::{t, tf};
use crate::core::launch::{self, LaunchAccount, LaunchRequest};
use crate::gui::app::{App, Message, Task};

impl App {
    pub(crate) fn launch_page(&mut self, ui: &mut egui::Ui) {
        ui.heading(t("Launch instance"));
        ui.horizontal(|ui| {
            ui.label(t("Minecraft dir:"));
            ui.add(egui::TextEdit::singleline(&mut self.minecraft_dir).id_salt("launch_dir"));
            if ui.button(t("Scan installed")).clicked() {
                self.scan_installed();
            }
        });
        if let Some(error) = &self.installed_error {
            ui.label(tf("scan failed: {}", &[error.as_str()]));
        }
        if self.installed.is_empty() {
            ui.label(t("no installed instance found, install one first"));
        } else {
            ui.label(t("Installed instances:"));
            egui::ScrollArea::vertical()
                .id_salt("launch_instances")
                .max_height(160.0)
                .show(ui, |ui| {
                    for instance in &self.installed {
                        let label = match instance.loader {
                            Some(tag) => format!("{}  · {tag}", instance.id),
                            None => instance.id.clone(),
                        };
                        if ui
                            .selectable_label(self.launch_version == instance.id, label)
                            .clicked()
                        {
                            self.launch_version = instance.id.clone();
                        }
                    }
                });
        }
        let selected = self.launch_version.trim().to_owned();
        if !selected.is_empty() {
            let isolate = self.effective_isolate(&selected);
            let source = if self.instance_uses_global(&selected) {
                t("global")
            } else {
                t("instance")
            };
            let java = self
                .effective_java(&selected)
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| t("auto (match the game version)").to_owned());
            let isolation = if isolate { t("on") } else { t("off") };
            ui.label(
                egui::RichText::new(tf(
                    "Java: {}  ·  version isolation: {}  ({} config, edit in Settings)",
                    &[java.as_str(), isolation, source],
                ))
                .weak()
                .small(),
            );
        }
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if ui
                .add_enabled(
                    !self.launching && !self.launch_version.trim().is_empty(),
                    egui::Button::new(t("Launch")),
                )
                .clicked()
            {
                self.start_launch(ui.ctx());
            }
            if self.launching {
                ui.add(egui::widgets::Spinner::new().size(14.0));
            }
        });
    }

    pub(crate) fn start_launch(&mut self, ctx: &egui::Context) {
        let version = self.launch_version.trim().to_owned();
        if version.is_empty() {
            self.log
                .push(t("select an installed instance first").to_owned());
            return;
        }
        self.launching = true;
        let dir = PathBuf::from(self.minecraft_dir.trim());
        let java = self.effective_java(&version);
        let java_candidates = self.java_candidates();
        let isolate = self.effective_isolate(&version);
        let auth = Arc::clone(&self.auth);
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        self.handle.spawn(async move {
            let result = launch_task(
                &auth,
                &dir,
                &version,
                java.as_deref(),
                &java_candidates,
                isolate,
            )
            .await;
            let line = match result {
                Ok(pid) => tf(
                    "launched {}, pid {}",
                    &[version.as_str(), pid.to_string().as_str()],
                ),
                Err(error) => {
                    let error = error.to_string();
                    tf("launch failed: {}", &[error.as_str()])
                }
            };
            let _ = tx.send(Message::Log(line));
            let _ = tx.send(Message::TaskDone(Task::Launch));
            ctx.request_repaint();
        });
    }
}

async fn launch_task(
    auth: &Auth,
    minecraft_dir: &Path,
    version: &str,
    java: Option<&Path>,
    java_candidates: &[String],
    isolate: bool,
) -> Result<u32, String> {
    let session = auth
        .refresh_current()
        .await
        .map_err(|error| error.to_string())?;
    if session.state == SessionState::Offline {
        spawn_game(
            minecraft_dir,
            version,
            LaunchAccount::Offline(&session.profile),
            java,
            java_candidates,
            isolate,
        )
    } else {
        let token = auth
            .current_game_token()
            .await
            .map_err(|error| error.to_string())?;
        spawn_game(
            minecraft_dir,
            version,
            LaunchAccount::Microsoft(&token),
            java,
            java_candidates,
            isolate,
        )
    }
}

fn spawn_game(
    minecraft_dir: &Path,
    version: &str,
    account: LaunchAccount<'_>,
    java: Option<&Path>,
    java_candidates: &[String],
    isolate: bool,
) -> Result<u32, String> {
    let child = launch::launch(LaunchRequest {
        minecraft_dir,
        version_id: version,
        account,
        java,
        java_candidates,
        game_dir: None,
        isolated: isolate,
    })
    .map_err(|error| error.to_string())?;
    Ok(child.id())
}
