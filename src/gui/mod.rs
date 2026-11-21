use std::path::{Path, PathBuf};
use std::sync::{Arc, mpsc};

use eframe::egui;
use tokio::runtime::Handle;

use crate::core::auth::{AccountSession, Auth, SessionState};
use crate::core::install::{self, AvailableVersion, InstallProgress};
use crate::core::launch::{self, LaunchAccount, LaunchRequest};

const DEFAULT_MINECRAFT_DIR: &str = ".minecraft";

enum Message {
    Log(String),
    TaskDone,
    Progress(InstallProgress),
    Versions(Result<Vec<AvailableVersion>, String>),
}

pub fn run() -> Result<(), String> {
    let auth = Auth::new().map_err(|error| error.to_string())?;
    let handle = Handle::current();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([880.0, 620.0]),
        ..Default::default()
    };
    eframe::run_native(
        "LYYS Minecraft Launcher",
        options,
        Box::new(move |_cc| Ok(Box::new(App::new(auth, handle)))),
    )
    .map_err(|error| error.to_string())
}

struct App {
    auth: Arc<Auth>,
    handle: Handle,
    tx: mpsc::Sender<Message>,
    rx: mpsc::Receiver<Message>,
    accounts: Vec<AccountSession>,
    active: Option<String>,
    offline_name: String,
    version: String,
    versions: Vec<AvailableVersion>,
    versions_loading: bool,
    versions_error: Option<String>,
    versions_requested: bool,
    show_all_versions: bool,
    minecraft_dir: String,
    isolate: bool,
    progress: Option<InstallProgress>,
    log: Vec<String>,
    busy: bool,
}

impl App {
    fn new(auth: Auth, handle: Handle) -> Self {
        let (tx, rx) = mpsc::channel();
        let mut app = Self {
            auth: Arc::new(auth),
            handle,
            tx,
            rx,
            accounts: Vec::new(),
            active: None,
            offline_name: String::new(),
            version: String::new(),
            versions: Vec::new(),
            versions_loading: false,
            versions_error: None,
            versions_requested: false,
            show_all_versions: false,
            minecraft_dir: DEFAULT_MINECRAFT_DIR.to_owned(),
            isolate: false,
            progress: None,
            log: Vec::new(),
            busy: false,
        };
        app.reload_accounts();
        app
    }

    fn reload_accounts(&mut self) {
        match (self.auth.list(), self.auth.active_uuid()) {
            (Ok(accounts), Ok(active)) => {
                self.accounts = accounts;
                self.active = active;
            }
            (Err(error), _) | (_, Err(error)) => {
                self.log.push(format!("failed to read accounts: {error}"));
            }
        }
    }

    fn reload_versions(&mut self, ctx: &egui::Context) {
        self.versions_loading = true;
        self.versions_error = None;
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        self.handle.spawn(async move {
            let result = install::available_versions()
                .await
                .map_err(|error| error.to_string());
            let _ = tx.send(Message::Versions(result));
            ctx.request_repaint();
        });
    }

    fn game_dir(&self) -> PathBuf {
        PathBuf::from(self.minecraft_dir.trim())
    }

    fn start_login(&mut self, ctx: &egui::Context) {
        self.busy = true;
        let auth = Arc::clone(&self.auth);
        let tx = self.tx.clone();
        let prompt_tx = tx.clone();
        let ctx = ctx.clone();
        self.handle.spawn(async move {
            let result = auth
                .login(move |prompt| {
                    let _ = prompt_tx.send(Message::Log(format!(
                        "open {} and enter code {}",
                        prompt.verification_uri, prompt.user_code
                    )));
                })
                .await;
            let line = match result {
                Ok(session) => format!("logged in: {}", session.profile.name),
                Err(error) => format!("login failed: {error}"),
            };
            let _ = tx.send(Message::Log(line));
            let _ = tx.send(Message::TaskDone);
            ctx.request_repaint();
        });
    }

    fn start_install(&mut self, ctx: &egui::Context) {
        let version = self.version.trim().to_owned();
        if version.is_empty() {
            self.log.push("enter a version first".to_owned());
            return;
        }
        self.busy = true;
        self.progress = Some(InstallProgress {
            downloaded: 0,
            skipped: 0,
            current: None,
        });
        let dir = self.game_dir();
        let tx = self.tx.clone();
        let progress_tx = tx.clone();
        let ctx = ctx.clone();
        self.handle.spawn_blocking(move || {
            let mut reporter = |progress: InstallProgress| {
                let _ = progress_tx.send(Message::Progress(progress));
                ctx.request_repaint();
            };
            let line = match install::install_with_reporter(&dir, &version, &mut reporter) {
                Ok(_) => format!("installed {version}"),
                Err(error) => format!("install failed: {error}"),
            };
            let _ = tx.send(Message::Log(line));
            let _ = tx.send(Message::TaskDone);
            ctx.request_repaint();
        });
    }

    fn start_launch(&mut self, ctx: &egui::Context) {
        let version = self.version.trim().to_owned();
        if version.is_empty() {
            self.log.push("enter a version first".to_owned());
            return;
        }
        self.busy = true;
        self.progress = None;
        let dir = self.game_dir();
        let isolate = self.isolate;
        let auth = Arc::clone(&self.auth);
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        self.handle.spawn(async move {
            let result = launch_task(&auth, &dir, &version, isolate).await;
            let line = match result {
                Ok(pid) => format!("launched {version}, pid {pid}"),
                Err(error) => format!("launch failed: {error}"),
            };
            let _ = tx.send(Message::Log(line));
            let _ = tx.send(Message::TaskDone);
            ctx.request_repaint();
        });
    }
}

async fn launch_task(
    auth: &Auth,
    minecraft_dir: &Path,
    version: &str,
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
            isolate,
        )
    }
}

fn spawn_game(
    minecraft_dir: &Path,
    version: &str,
    account: LaunchAccount<'_>,
    isolate: bool,
) -> Result<u32, String> {
    let child = launch::launch(LaunchRequest {
        minecraft_dir,
        version_id: version,
        account,
        java: None,
        game_dir: None,
        isolated: isolate,
    })
    .map_err(|error| error.to_string())?;
    Ok(child.id())
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        while let Ok(message) = self.rx.try_recv() {
            match message {
                Message::Log(line) => self.log.push(line),
                Message::TaskDone => {
                    self.busy = false;
                    self.reload_accounts();
                }
                Message::Progress(progress) => self.progress = Some(progress),
                Message::Versions(result) => {
                    self.versions_loading = false;
                    match result {
                        Ok(versions) => {
                            self.versions = versions;
                            self.versions_error = None;
                            if self.version.trim().is_empty() {
                                if let Some(latest) =
                                    self.versions.iter().find(|entry| entry.kind == "release")
                                {
                                    self.version = latest.id.clone();
                                }
                            }
                        }
                        Err(error) => self.versions_error = Some(error),
                    }
                }
            }
        }

        if !self.versions_requested {
            self.versions_requested = true;
            self.reload_versions(&ctx);
        }

        egui::Panel::top("accounts").show(ui, |ui| {
            ui.add_space(6.0);
            ui.heading("Accounts");
            for index in 0..self.accounts.len() {
                let uuid = self.accounts[index].profile.id.clone();
                let name = self.accounts[index].profile.name.clone();
                let state = self.accounts[index].state;
                let is_active = self.active.as_deref() == Some(uuid.as_str());
                ui.horizontal(|ui| {
                    if ui.selectable_label(is_active, name).clicked() {
                        match self.auth.set_active(&uuid) {
                            Ok(()) => self.active = Some(uuid.clone()),
                            Err(error) => self
                                .log
                                .push(format!("failed to set active account: {error}")),
                        }
                    }
                    ui.label(format!("{state:?}"));
                    if ui.small_button("Remove").clicked() {
                        match self.auth.remove(&uuid) {
                            Ok(()) => self.reload_accounts(),
                            Err(error) => {
                                self.log.push(format!("failed to remove account: {error}"))
                            }
                        }
                    }
                });
            }
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label("Offline name:");
                ui.text_edit_singleline(&mut self.offline_name);
                if ui
                    .add_enabled(!self.busy, egui::Button::new("Add offline"))
                    .clicked()
                {
                    let name = self.offline_name.trim().to_owned();
                    match self.auth.add_offline(&name) {
                        Ok(_) => {
                            self.offline_name.clear();
                            self.reload_accounts();
                        }
                        Err(error) => self.log.push(format!("failed to add account: {error}")),
                    }
                }
                if ui
                    .add_enabled(!self.busy, egui::Button::new("Login with Microsoft"))
                    .clicked()
                {
                    self.start_login(&ctx);
                }
            });
            ui.add_space(6.0);
        });

        egui::Panel::bottom("log")
            .resizable(true)
            .min_size(120.0)
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        for line in &self.log {
                            ui.monospace(line);
                        }
                    });
            });

        egui::CentralPanel::default().show(ui, |ui| {
            ui.heading("Game");
            ui.horizontal(|ui| {
                ui.label("Version:");
                ui.text_edit_singleline(&mut self.version);
                if ui
                    .add_enabled(!self.versions_loading, egui::Button::new("Refresh list"))
                    .clicked()
                {
                    self.reload_versions(&ctx);
                }
            });
            ui.horizontal(|ui| {
                ui.checkbox(
                    &mut self.show_all_versions,
                    "Show snapshots and old versions",
                );
                if self.versions_loading {
                    ui.add(egui::widgets::Spinner::new().size(14.0));
                    ui.label("loading versions...");
                } else if let Some(error) = &self.versions_error {
                    ui.label(format!("version list failed: {error}"));
                }
            });
            if !self.versions.is_empty() {
                egui::ScrollArea::vertical()
                    .max_height(140.0)
                    .show(ui, |ui| {
                        for version in &self.versions {
                            if !self.show_all_versions && version.kind != "release" {
                                continue;
                            }
                            let selected = self.version == version.id;
                            if ui
                                .selectable_label(
                                    selected,
                                    format!("{}  ({})", version.id, version.kind),
                                )
                                .clicked()
                            {
                                self.version = version.id.clone();
                            }
                        }
                    });
            }
            ui.horizontal(|ui| {
                ui.label("Minecraft dir:");
                ui.text_edit_singleline(&mut self.minecraft_dir);
            });
            ui.checkbox(&mut self.isolate, "Isolated game dir (versions/<version>)");
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(!self.busy, egui::Button::new("Install"))
                    .clicked()
                {
                    self.start_install(&ctx);
                }
                if ui
                    .add_enabled(!self.busy, egui::Button::new("Launch"))
                    .clicked()
                {
                    self.start_launch(&ctx);
                }
            });
            if let Some(progress) = &self.progress {
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if self.busy {
                        ui.add(egui::widgets::Spinner::new().size(14.0));
                    }
                    ui.monospace(format!(
                        "files downloaded: {} · skipped: {}",
                        progress.downloaded, progress.skipped
                    ));
                });
                if let Some(current) = &progress.current {
                    ui.monospace(format!("current: {current}"));
                }
            }
        });
    }
}
