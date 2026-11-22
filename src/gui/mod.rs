use std::path::{Path, PathBuf};
use std::sync::{Arc, mpsc};

use eframe::egui;
use tokio::runtime::Handle;

use crate::core::auth::{AccountSession, Auth, SessionState};
use crate::core::install::{self, AvailableVersion, InstallProgress};
use crate::core::launch::{self, LaunchAccount, LaunchRequest};

const DEFAULT_MINECRAFT_DIR: &str = ".minecraft";

const CONTOUR_SEGMENTS: usize = 96;

/// One topographic "hill": a stack of nested, wobbly closed rings.
struct ContourHill {
    /// Center in normalized panel coordinates (0..=1).
    center: (f32, f32),
    /// Radius of the innermost ring, as a fraction of the panel's short side.
    inner: f32,
    /// Distance between neighbouring rings, same unit as `inner`.
    spacing: f32,
    rings: usize,
    /// Radius modulation amplitudes at 2x, 3x and 5x the angle.
    wobble: (f32, f32, f32),
    phase: f32,
    /// Horizontal stretch before rotation.
    stretch: f32,
    rotation: f32,
}

const CONTOUR_HILLS: [ContourHill; 3] = [
    ContourHill {
        center: (0.06, 0.94),
        inner: 0.05,
        spacing: 0.055,
        rings: 7,
        wobble: (0.10, 0.05, 0.03),
        phase: 0.4,
        stretch: 1.25,
        rotation: 0.35,
    },
    ContourHill {
        center: (0.92, 0.06),
        inner: 0.04,
        spacing: 0.045,
        rings: 6,
        wobble: (0.09, 0.06, 0.02),
        phase: 2.1,
        stretch: 1.15,
        rotation: -0.5,
    },
    ContourHill {
        center: (0.62, 0.98),
        inner: 0.03,
        spacing: 0.05,
        rings: 5,
        wobble: (0.12, 0.04, 0.03),
        phase: 4.0,
        stretch: 1.1,
        rotation: 0.8,
    },
];

/// Paint faint contour rings across the given rect.
fn paint_contour_background(painter: &egui::Painter, rect: egui::Rect, visuals: &egui::Visuals) {
    let stroke = egui::Stroke::new(1.0, visuals.weak_text_color().gamma_multiply(0.35));
    let short = rect.width().min(rect.height());
    let mut shapes = Vec::new();
    for hill in &CONTOUR_HILLS {
        let center = rect.lerp_inside(egui::vec2(hill.center.0, hill.center.1));
        let (sin_rot, cos_rot) = hill.rotation.sin_cos();
        for ring in 0..hill.rings {
            let radius = (hill.inner + hill.spacing * ring as f32) * short;
            let mut points = Vec::with_capacity(CONTOUR_SEGMENTS);
            for step in 0..CONTOUR_SEGMENTS {
                let angle = step as f32 / CONTOUR_SEGMENTS as f32 * std::f32::consts::TAU;
                let (sin_a, cos_a) = angle.sin_cos();
                let wobble = 1.0
                    + hill.wobble.0 * (2.0 * angle + hill.phase).sin()
                    + hill.wobble.1 * (3.0 * angle + hill.phase * 0.7).sin()
                    + hill.wobble.2 * (5.0 * angle + hill.phase * 1.9).sin();
                let x = cos_a * radius * wobble * hill.stretch;
                let y = sin_a * radius * wobble;
                points.push(
                    center + egui::vec2(x * cos_rot - y * sin_rot, x * sin_rot + y * cos_rot),
                );
            }
            shapes.push(egui::Shape::closed_line(points, stroke));
        }
    }
    painter.extend(shapes);
}

#[derive(Clone, Copy)]
enum Task {
    Login,
    Install,
    Launch,
}

enum Message {
    Log(String),
    TaskDone(Task),
    Progress(InstallProgress),
    Versions(Result<Vec<AvailableVersion>, String>),
}

#[derive(PartialEq, Eq, Clone, Copy)]
enum Page {
    Install,
    Launch,
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
    page: Page,
    // 安装实例
    install_version: String,
    install_dir: String,
    versions: Vec<AvailableVersion>,
    versions_loading: bool,
    versions_error: Option<String>,
    versions_requested: bool,
    show_all_versions: bool,
    install_progress: Option<InstallProgress>,
    installing: bool,
    // 启动实例
    launch_version: String,
    launch_dir: String,
    installed: Vec<String>,
    installed_error: Option<String>,
    launching: bool,
    isolate: bool,
    logging_in: bool,
    log: Vec<String>,
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
            page: Page::Install,
            install_version: String::new(),
            install_dir: DEFAULT_MINECRAFT_DIR.to_owned(),
            versions: Vec::new(),
            versions_loading: false,
            versions_error: None,
            versions_requested: false,
            show_all_versions: false,
            install_progress: None,
            installing: false,
            launch_version: String::new(),
            launch_dir: DEFAULT_MINECRAFT_DIR.to_owned(),
            installed: Vec::new(),
            installed_error: None,
            launching: false,
            isolate: false,
            logging_in: false,
            log: Vec::new(),
        };
        app.reload_accounts();
        app.scan_installed();
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

    fn scan_installed(&mut self) {
        let dir = PathBuf::from(self.launch_dir.trim());
        match install::installed_versions(&dir) {
            Ok(versions) => {
                self.installed_error = None;
                if !versions.iter().any(|id| id == &self.launch_version) {
                    self.launch_version = versions.first().cloned().unwrap_or_default();
                }
                self.installed = versions;
            }
            Err(error) => self.installed_error = Some(error.to_string()),
        }
    }

    fn start_login(&mut self, ctx: &egui::Context) {
        self.logging_in = true;
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
            let _ = tx.send(Message::TaskDone(Task::Login));
            ctx.request_repaint();
        });
    }

    fn start_install(&mut self, ctx: &egui::Context) {
        let version = self.install_version.trim().to_owned();
        if version.is_empty() {
            self.log.push("enter a version first".to_owned());
            return;
        }
        self.installing = true;
        self.install_progress = Some(InstallProgress {
            downloaded: 0,
            skipped: 0,
            current: None,
        });
        let dir = PathBuf::from(self.install_dir.trim());
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
            let _ = tx.send(Message::TaskDone(Task::Install));
            ctx.request_repaint();
        });
    }

    fn start_launch(&mut self, ctx: &egui::Context) {
        let version = self.launch_version.trim().to_owned();
        if version.is_empty() {
            self.log.push("select an installed instance first".to_owned());
            return;
        }
        self.launching = true;
        let dir = PathBuf::from(self.launch_dir.trim());
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
            let _ = tx.send(Message::TaskDone(Task::Launch));
            ctx.request_repaint();
        });
    }

    fn install_page(&mut self, ui: &mut egui::Ui) {
        ui.heading("Install instance");
        ui.horizontal(|ui| {
            ui.label("Version:");
            ui.text_edit_singleline(&mut self.install_version);
            if ui
                .add_enabled(!self.versions_loading, egui::Button::new("Refresh list"))
                .clicked()
            {
                self.reload_versions(ui.ctx());
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
                        let selected = self.install_version == version.id;
                        if ui
                            .selectable_label(
                                selected,
                                format!("{}  ({})", version.id, version.kind),
                            )
                            .clicked()
                        {
                            self.install_version = version.id.clone();
                        }
                    }
                });
        }
        ui.horizontal(|ui| {
            ui.label("Minecraft dir:");
            ui.text_edit_singleline(&mut self.install_dir);
        });
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if ui
                .add_enabled(!self.installing, egui::Button::new("Install"))
                .clicked()
            {
                self.start_install(ui.ctx());
            }
            if self.installing {
                ui.add(egui::widgets::Spinner::new().size(14.0));
            }
        });
        if let Some(progress) = &self.install_progress {
            ui.add_space(6.0);
            ui.monospace(format!(
                "files downloaded: {} · skipped: {}",
                progress.downloaded, progress.skipped
            ));
            if let Some(current) = &progress.current {
                ui.monospace(format!("current: {current}"));
            }
        }
    }

    fn launch_page(&mut self, ui: &mut egui::Ui) {
        ui.heading("Launch instance");
        ui.horizontal(|ui| {
            ui.label("Minecraft dir:");
            ui.text_edit_singleline(&mut self.launch_dir);
            if ui.button("Scan installed").clicked() {
                self.scan_installed();
            }
        });
        if let Some(error) = &self.installed_error {
            ui.label(format!("scan failed: {error}"));
        }
        if self.installed.is_empty() {
            ui.label("no installed instance found, install one first");
        } else {
            ui.label("Installed instances:");
            egui::ScrollArea::vertical()
                .max_height(160.0)
                .show(ui, |ui| {
                    for id in &self.installed {
                        if ui
                            .selectable_label(self.launch_version == id.as_str(), id.as_str())
                            .clicked()
                        {
                            self.launch_version = id.clone();
                        }
                    }
                });
        }
        ui.checkbox(&mut self.isolate, "Isolated game dir (versions/<version>)");
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if ui
                .add_enabled(
                    !self.launching && !self.launch_version.trim().is_empty(),
                    egui::Button::new("Launch"),
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
                Message::TaskDone(task) => match task {
                    Task::Login => {
                        self.logging_in = false;
                        self.reload_accounts();
                    }
                    Task::Install => {
                        self.installing = false;
                        self.scan_installed();
                    }
                    Task::Launch => self.launching = false,
                },
                Message::Progress(progress) => self.install_progress = Some(progress),
                Message::Versions(result) => {
                    self.versions_loading = false;
                    match result {
                        Ok(versions) => {
                            self.versions = versions;
                            self.versions_error = None;
                            if self.install_version.trim().is_empty() {
                                if let Some(latest) =
                                    self.versions.iter().find(|entry| entry.kind == "release")
                                {
                                    self.install_version = latest.id.clone();
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
                    .add_enabled(!self.logging_in, egui::Button::new("Add offline"))
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
                    .add_enabled(!self.logging_in, egui::Button::new("Login with Microsoft"))
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
            paint_contour_background(ui.painter(), ui.max_rect(), ui.visuals());
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.page, Page::Install, "Install instance");
                ui.selectable_value(&mut self.page, Page::Launch, "Launch instance");
            });
            ui.separator();
            match self.page {
                Page::Install => self.install_page(ui),
                Page::Launch => self.launch_page(ui),
            }
        });
    }
}
