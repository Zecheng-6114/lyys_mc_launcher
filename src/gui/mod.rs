use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, mpsc};

use eframe::egui;
use serde::{Deserialize, Serialize};
use tokio::runtime::Handle;

use mc_launcher_core::loader::LoaderKind;
use mc_launcher_core::loader::common::{LoaderSpec, LoaderVersion};
use mc_launcher_core::utils::get_java_executable;

use crate::core::auth::{AccountSession, Auth, SessionState};
use crate::core::config::{self, InstanceConfig, LauncherConfig};
use crate::core::install::{self, AvailableVersion, InstallProgress};
use crate::core::launch::{self, LaunchAccount, LaunchRequest};
use crate::core::modrinth::{
    self, InstallOutcome, ModSearchHit, ModSearchRequest, ModSearchResult, ProjectType,
};

const DEFAULT_MINECRAFT_DIR: &str = ".minecraft";
/// Records which jar each installed mod slug maps to, so the UI can tell that a mod is installed.
const MOD_MANIFEST_FILE: &str = ".lyys_mods.json";

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
    LoaderVersions(Result<Vec<String>, String>),
    ModSearch {
        page: u32,
        result: Result<ModSearchResult, String>,
    },
    ModInstalled {
        dir: PathBuf,
        game_version: String,
        loader: String,
        kind: ProjectType,
        result: Result<InstallOutcome, String>,
    },
    ModUpdates {
        dir: PathBuf,
        updates: HashMap<String, (String, String)>,
    },
}

#[derive(PartialEq, Eq, Clone, Copy)]
enum Page {
    Install,
    Launch,
    Mods,
    Settings,
}

#[derive(PartialEq, Eq, Clone, Copy)]
enum LoaderChoice {
    Vanilla,
    Fabric,
    Quilt,
    Forge,
    NeoForge,
}

impl LoaderChoice {
    const ALL: [Self; 5] = [
        Self::Vanilla,
        Self::Fabric,
        Self::Quilt,
        Self::Forge,
        Self::NeoForge,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::Vanilla => "Vanilla",
            Self::Fabric => "Fabric",
            Self::Quilt => "Quilt",
            Self::Forge => "Forge",
            Self::NeoForge => "NeoForge",
        }
    }

    fn to_spec(self, version: &str) -> Option<LoaderSpec> {
        let version = version.trim();
        let version = if version.is_empty() {
            LoaderVersion::Latest
        } else {
            LoaderVersion::Exact(version.to_owned())
        };
        match self {
            Self::Vanilla => None,
            Self::Fabric => Some(LoaderSpec::Fabric { version }),
            Self::Quilt => Some(LoaderSpec::Quilt { version }),
            Self::Forge => Some(LoaderSpec::Forge { version }),
            Self::NeoForge => Some(LoaderSpec::NeoForge { version }),
        }
    }

    fn kind(self) -> Option<LoaderKind> {
        match self {
            Self::Vanilla => None,
            Self::Fabric => Some(LoaderKind::Fabric),
            Self::Quilt => Some(LoaderKind::Quilt),
            Self::Forge => Some(LoaderKind::Forge),
            Self::NeoForge => Some(LoaderKind::NeoForge),
        }
    }
}

pub fn run() -> Result<(), String> {
    let auth = Auth::new().map_err(|error| error.to_string())?;
    let handle = Handle::current();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([880.0, 620.0]),
        ..Default::default()
    };
    eframe::run_native(
        "LYYS MC Launcher",
        options,
        Box::new(move |cc| {
            egui_extras::install_image_loaders(&cc.egui_ctx);
            Ok(Box::new(App::new(auth, handle)))
        }),
    )
    .map_err(|error| error.to_string())
}

#[derive(Clone)]
struct InstalledInstance {
    id: String,
    loader: Option<&'static str>,
    game_version: Option<String>,
}

/// What the manifest remembers about one installed mod.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct ModRecord {
    filename: String,
    #[serde(default)]
    version_id: String,
    #[serde(default)]
    version_number: String,
    #[serde(default)]
    game_version: String,
    #[serde(default)]
    loader: String,
    /// True when the jar was pulled in as a required dependency.
    #[serde(default)]
    auto: bool,
    /// Project ids of the required dependencies.
    #[serde(default)]
    dependencies: Vec<String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ModAction {
    Install,
    Reinstall,
    Uninstall,
    Update,
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
    minecraft_dir: String,
    install_loader: LoaderChoice,
    install_loader_version: String,
    loader_versions: Vec<String>,
    loader_versions_loading: bool,
    loader_versions_error: Option<String>,
    loader_versions_key: Option<(LoaderChoice, String)>,
    java_path: String,
    versions: Vec<AvailableVersion>,
    versions_loading: bool,
    versions_error: Option<String>,
    versions_requested: bool,
    show_all_versions: bool,
    install_progress: Option<InstallProgress>,
    installing: bool,
    // 启动实例
    launch_version: String,
    installed: Vec<InstalledInstance>,
    installed_dir: String,
    installed_error: Option<String>,
    launching: bool,
    isolate: bool,
    // 持久化配置
    config: LauncherConfig,
    config_status: Option<String>,
    // 搜索模组
    mod_query: String,
    mod_kind: ProjectType,
    mod_minecraft_version: String,
    mod_loader: Option<LoaderKind>,
    mod_instance_id: Option<String>,
    mod_datapack_world: String,
    datapack_worlds: Vec<String>,
    datapack_worlds_dir: String,
    mod_results: Vec<ModSearchHit>,
    mod_total: u64,
    /// 当前页码，从 1 开始。
    mod_page: u32,
    mod_searching: bool,
    mod_installing: Option<String>,
    mod_notice: Option<String>,
    mod_error: Option<String>,
    installed_mods: HashMap<String, ModRecord>,
    installed_mods_dir: String,
    installed_kind: Option<ProjectType>,
    mod_updates: HashMap<String, (String, String)>,
    logging_in: bool,
    log: Vec<String>,
}

impl App {
    fn new(auth: Auth, handle: Handle) -> Self {
        let (tx, rx) = mpsc::channel();
        let (config, config_status) = match LauncherConfig::load() {
            Ok(config) => (config, None),
            Err(error) => (
                LauncherConfig::default(),
                Some(format!("failed to load configuration: {error}")),
            ),
        };
        let minecraft_dir = if config.minecraft_dir.trim().is_empty() {
            DEFAULT_MINECRAFT_DIR.to_owned()
        } else {
            config.minecraft_dir.trim().to_owned()
        };
        let java_path = if config.java_path.trim().is_empty() {
            get_java_executable().unwrap_or_else(|| "java".to_owned())
        } else {
            config.java_path.trim().to_owned()
        };
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
            minecraft_dir,
            install_loader: LoaderChoice::Vanilla,
            install_loader_version: String::new(),
            loader_versions: Vec::new(),
            loader_versions_loading: false,
            loader_versions_error: None,
            loader_versions_key: None,
            java_path,
            versions: Vec::new(),
            versions_loading: false,
            versions_error: None,
            versions_requested: false,
            show_all_versions: false,
            install_progress: None,
            installing: false,
            launch_version: String::new(),
            installed: Vec::new(),
            installed_dir: String::new(),
            installed_error: None,
            launching: false,
            isolate: config.isolate,
            config,
            config_status,
            mod_query: String::new(),
            mod_kind: ProjectType::Mod,
            mod_minecraft_version: String::new(),
            mod_loader: None,
            mod_instance_id: None,
            mod_datapack_world: String::new(),
            datapack_worlds: Vec::new(),
            datapack_worlds_dir: String::new(),
            mod_results: Vec::new(),
            mod_total: 0,
            mod_page: 1,
            mod_searching: false,
            mod_installing: None,
            mod_notice: None,
            mod_error: None,
            installed_mods: HashMap::new(),
            installed_mods_dir: String::new(),
            installed_kind: None,
            mod_updates: HashMap::new(),
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

    fn reload_loader_versions(&mut self, ctx: &egui::Context) {
        let Some(kind) = self.install_loader.kind() else {
            return;
        };
        self.loader_versions_loading = true;
        self.loader_versions_error = None;
        self.loader_versions.clear();
        let minecraft_version = self.install_version.trim().to_owned();
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        self.handle.spawn_blocking(move || {
            let result = install::loader_versions(kind, &minecraft_version)
                .map_err(|error| error.to_string());
            let _ = tx.send(Message::LoaderVersions(result));
            ctx.request_repaint();
        });
    }

    fn scan_installed(&mut self) {
        let dir = PathBuf::from(self.minecraft_dir.trim());
        self.installed_dir = self.minecraft_dir.trim().to_owned();
        match install::installed_versions(&dir) {
            Ok(versions) => {
                self.installed_error = None;
                let instances: Vec<InstalledInstance> = versions
                    .into_iter()
                    .map(|id| {
                        let (loader, game_version) = inspect_instance(&dir, &id);
                        InstalledInstance {
                            id,
                            loader,
                            game_version,
                        }
                    })
                    .collect();
                if !instances
                    .iter()
                    .any(|instance| instance.id == self.launch_version)
                {
                    self.launch_version =
                        instances.first().map(|instance| instance.id.clone()).unwrap_or_default();
                }
                // 目录切换后，之前选中的模组安装目标可能已不存在
                let stale_target = self.mod_instance_id.as_ref().is_some_and(|target| {
                    !instances
                        .iter()
                        .any(|instance| &instance.id == target)
                });
                if stale_target {
                    self.mod_instance_id = None;
                }
                self.installed = instances;
            }
            Err(error) => {
                self.installed.clear();
                self.installed_error = Some(error.to_string());
            }
        }
    }

    /// The instance whose mods directory should receive downloads when isolation is on.
    fn mods_target_instance(&self) -> Option<&str> {
        self.mod_instance_id.as_deref().or_else(|| {
            let launch = self.launch_version.trim();
            (!launch.is_empty()).then_some(launch)
        })
    }

    /// Whether `instance` falls back to the global configuration.
    fn instance_uses_global(&self, instance: &str) -> bool {
        self.config
            .instances
            .get(instance)
            .map(|config| config.use_global)
            .unwrap_or(true)
    }

    /// Version isolation for `instance`: its own config, else the global toggle.
    fn effective_isolate(&self, instance: &str) -> bool {
        match self.config.instances.get(instance) {
            Some(config) if !config.use_global => config.isolate,
            _ => self.isolate,
        }
    }

    /// Java executable for `instance`: its own config, else the global one.
    fn effective_java(&self, instance: &str) -> PathBuf {
        let configured = match self.config.instances.get(instance) {
            Some(config) if !config.use_global => config.java_path.trim(),
            _ => "",
        };
        let path = if configured.is_empty() {
            self.java_path.trim()
        } else {
            configured
        };
        if path.is_empty() {
            PathBuf::from("java")
        } else {
            PathBuf::from(path)
        }
    }

    /// Isolation that applies to the current mods target.
    fn mods_isolate(&self) -> bool {
        self.mods_target_instance()
            .map_or(self.isolate, |instance| self.effective_isolate(instance))
    }

    /// Game directory: `versions/<instance>` when version isolation is on.
    fn game_dir(&self) -> PathBuf {
        let base = PathBuf::from(self.minecraft_dir.trim());
        match self.mods_target_instance() {
            Some(instance) if self.mods_isolate() => base.join("versions").join(instance),
            _ => base,
        }
    }

    /// Directory that receives downloads of `kind`.
    fn content_dir(&self, kind: ProjectType) -> PathBuf {
        match kind {
            ProjectType::Mod => self.game_dir().join("mods"),
            ProjectType::ResourcePack => self.game_dir().join("resourcepacks"),
            ProjectType::ShaderPack => self.game_dir().join("shaderpacks"),
            ProjectType::DataPack => self
                .game_dir()
                .join("saves")
                .join(self.mod_datapack_world.trim())
                .join("datapacks"),
        }
    }

    /// Scan `saves/` for worlds that can receive data packs.
    fn scan_worlds(&mut self, game_dir: &Path) {
        self.datapack_worlds_dir = game_dir.display().to_string();
        self.datapack_worlds.clear();
        if let Ok(entries) = std::fs::read_dir(game_dir.join("saves")) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() && path.join("level.dat").is_file() {
                    if let Some(name) = path.file_name().and_then(|name| name.to_str()) {
                        self.datapack_worlds.push(name.to_owned());
                    }
                }
            }
        }
        self.datapack_worlds.sort();
        if !self
            .datapack_worlds
            .iter()
            .any(|world| world == &self.mod_datapack_world)
        {
            self.mod_datapack_world = self.datapack_worlds.first().cloned().unwrap_or_default();
        }
    }

    fn manifest_file(kind: ProjectType) -> &'static str {
        match kind {
            ProjectType::Mod => MOD_MANIFEST_FILE,
            _ => ".lyys_content.json",
        }
    }

    /// Persist the configuration, syncing the runtime global values into it.
    fn save_config(&mut self) {
        self.config.minecraft_dir = self.minecraft_dir.trim().to_owned();
        self.config.java_path = self.java_path.trim().to_owned();
        self.config.isolate = self.isolate;
        match self.config.save() {
            Ok(path) => {
                self.config_status = Some(format!("saved to {}", path.display()));
                self.scan_installed();
            }
            Err(error) => self.config_status = Some(format!("save failed: {error}")),
        }
    }

    /// Read the manifest for `dir`, dropping entries whose file is gone.
    fn load_installed_mods(&mut self, dir: &Path, kind: ProjectType) {
        self.installed_mods_dir = dir.display().to_string();
        self.installed_kind = Some(kind);
        self.installed_mods.clear();
        self.mod_updates.clear();
        let Ok(text) = std::fs::read_to_string(dir.join(Self::manifest_file(kind))) else {
            return;
        };
        if let Ok(manifest) = serde_json::from_str::<HashMap<String, ModRecord>>(&text) {
            self.installed_mods.extend(
                manifest
                    .into_iter()
                    .filter(|(_, record)| dir.join(&record.filename).is_file()),
            );
            return;
        }
        if kind != ProjectType::Mod {
            return;
        }
        // 旧清单只记录了 slug -> 文件名，读进来至少还能显示为已安装
        if let Ok(legacy) = serde_json::from_str::<HashMap<String, String>>(&text) {
            for (key, filename) in legacy {
                if dir.join(&filename).is_file() {
                    self.installed_mods.insert(
                        key,
                        ModRecord {
                            filename,
                            ..Default::default()
                        },
                    );
                }
            }
        }
    }

    /// Ask Modrinth whether any installed project has a newer compatible version.
    fn start_update_check(&mut self, ctx: &egui::Context, dir: &Path, kind: ProjectType) {
        self.mod_updates.clear();
        let needs_loader = kind == ProjectType::Mod;
        let jobs: Vec<(String, String, String, String)> = self
            .installed_mods
            .iter()
            .filter(|(_, record)| {
                !record.version_id.is_empty()
                    && !record.game_version.trim().is_empty()
                    && (!needs_loader || !record.loader.trim().is_empty())
            })
            .map(|(key, record)| {
                (
                    key.clone(),
                    record.version_id.clone(),
                    record.game_version.clone(),
                    record.loader.clone(),
                )
            })
            .collect();
        if jobs.is_empty() {
            return;
        }
        let dir = dir.to_path_buf();
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        self.handle.spawn(async move {
            let mut updates = HashMap::new();
            for (key, current, game_version, loader) in jobs {
                let loader = (kind == ProjectType::Mod).then_some(loader.as_str());
                if let Ok(Some((version_id, number))) =
                    modrinth::latest_version(&key, &game_version, loader).await
                {
                    if version_id != current {
                        updates.insert(key, (version_id, number));
                    }
                }
            }
            let _ = tx.send(Message::ModUpdates { dir, updates });
            ctx.request_repaint();
        });
    }

    fn uninstall_mod(&mut self, key: &str, dir: &Path, kind: ProjectType) {
        if self.mod_installing.is_some() {
            return;
        }
        self.mod_error = None;
        self.mod_notice = None;
        let Some(record) = self.installed_mods.remove(key) else {
            return;
        };
        self.mod_updates.remove(key);
        let mut removed = 0usize;
        match std::fs::remove_file(dir.join(&record.filename)) {
            Ok(()) => removed += 1,
            Err(error) => {
                self.mod_error = Some(format!("failed to remove {}: {error}", record.filename))
            }
        }
        removed += self.remove_unused_dependencies(&record.dependencies, dir);
        self.save_installed_mods(dir, kind);
        if self.mod_error.is_none() {
            self.mod_notice = Some(format!(
                "uninstalled {}, removed {removed} file(s)",
                record.filename
            ));
        }
    }

    /// Remove auto-installed dependencies that are no longer required by another mod.
    fn remove_unused_dependencies(&mut self, dependencies: &[String], dir: &Path) -> usize {
        let mut removed = 0usize;
        for key in dependencies {
            let Some(record) = self.installed_mods.get(key).cloned() else {
                continue;
            };
            // 手动安装的前置保留
            if !record.auto || self.dependency_in_use(key) {
                continue;
            }
            self.installed_mods.remove(key);
            self.mod_updates.remove(key);
            if std::fs::remove_file(dir.join(&record.filename)).is_ok() {
                removed += 1;
            }
            removed += self.remove_unused_dependencies(&record.dependencies, dir);
        }
        removed
    }

    fn dependency_in_use(&self, key: &str) -> bool {
        self.installed_mods.values().any(|record| {
            record
                .dependencies
                .iter()
                .any(|dependency| dependency == key)
        })
    }

    fn save_installed_mods(&mut self, dir: &Path, kind: ProjectType) {
        match serde_json::to_string_pretty(&self.installed_mods) {
            Ok(text) => {
                if let Err(error) = std::fs::write(dir.join(Self::manifest_file(kind)), text) {
                    self.log
                        .push(format!("failed to write content manifest: {error}"));
                }
            }
            Err(error) => self
                .log
                .push(format!("failed to encode content manifest: {error}")),
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
        let dir = PathBuf::from(self.minecraft_dir.trim());
        let java = if self.java_path.trim().is_empty() {
            PathBuf::from("java")
        } else {
            PathBuf::from(self.java_path.trim())
        };
        let loader_name = self.install_loader.label();
        let loader = self.install_loader.to_spec(&self.install_loader_version);
        let tx = self.tx.clone();
        let progress_tx = tx.clone();
        let ctx = ctx.clone();
        self.handle.spawn_blocking(move || {
            let mut reporter = |progress: InstallProgress| {
                let _ = progress_tx.send(Message::Progress(progress));
                ctx.request_repaint();
            };
            let line = match loader {
                Some(loader) => {
                    match install::install_loader_with_reporter(
                        &dir,
                        &version,
                        loader,
                        &java,
                        &mut reporter,
                    ) {
                        Ok(result) => {
                            format!("installed {loader_name} {}", result.version_id)
                        }
                        Err(error) => format!("install failed: {error}"),
                    }
                }
                None => match install::install_with_reporter(&dir, &version, &mut reporter) {
                    Ok(_) => format!("installed {version}"),
                    Err(error) => format!("install failed: {error}"),
                },
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
        let dir = PathBuf::from(self.minecraft_dir.trim());
        let java = self.effective_java(&version);
        let isolate = self.effective_isolate(&version);
        let auth = Arc::clone(&self.auth);
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        self.handle.spawn(async move {
            let result = launch_task(&auth, &dir, &version, &java, isolate).await;
            let line = match result {
                Ok(pid) => format!("launched {version}, pid {pid}"),
                Err(error) => format!("launch failed: {error}"),
            };
            let _ = tx.send(Message::Log(line));
            let _ = tx.send(Message::TaskDone(Task::Launch));
            ctx.request_repaint();
        });
    }

    fn start_mod_search(&mut self, ctx: &egui::Context) {
        if self.mod_searching {
            return;
        }
        self.mod_page = 1;
        self.mod_results.clear();
        self.mod_total = 0;
        self.mod_notice = None;
        self.request_mod_page(1, ctx);
    }

    /// 请求指定页的结果，返回后整页替换当前列表。
    fn request_mod_page(&mut self, page: u32, ctx: &egui::Context) {
        if self.mod_searching {
            return;
        }
        self.mod_searching = true;
        self.mod_error = None;
        let query = self.mod_query.trim().to_owned();
        let minecraft_version = self.mod_minecraft_version.trim().to_owned();
        let kind = self.mod_kind;
        let loader = if kind == ProjectType::Mod {
            self.mod_loader.map(loader_slug)
        } else {
            None
        };
        let offset = (page - 1) * modrinth::PAGE_SIZE;
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        self.handle.spawn(async move {
            let game_version =
                (!minecraft_version.is_empty()).then_some(minecraft_version.as_str());
            let result = modrinth::search(ModSearchRequest {
                query: &query,
                project_type: kind,
                game_version,
                loader,
                offset,
            })
            .await
            .map_err(|error| error.to_string());
            let _ = tx.send(Message::ModSearch { page, result });
            ctx.request_repaint();
        });
    }

    fn goto_mod_page(&mut self, page: u32, ctx: &egui::Context) {
        if page == 0 || page == self.mod_page {
            return;
        }
        self.request_mod_page(page, ctx);
    }

    fn start_mod_install(
        &mut self,
        project: String,
        game_version: String,
        loader: Option<String>,
        kind: ProjectType,
        ctx: &egui::Context,
    ) {
        if game_version.trim().is_empty() {
            self.mod_error = Some("enter a Minecraft version first".to_owned());
            return;
        }
        if kind == ProjectType::Mod && loader.as_deref().unwrap_or("").trim().is_empty() {
            self.mod_error = Some("pick a loader before installing mods".to_owned());
            return;
        }
        if kind == ProjectType::DataPack && self.mod_datapack_world.trim().is_empty() {
            self.mod_error = Some("pick a world before installing data packs".to_owned());
            return;
        }
        if self.mods_isolate() && self.mods_target_instance().is_none() {
            self.mod_error =
                Some("version isolation is on, pick an instance to install into".to_owned());
            return;
        }
        let target = self.content_dir(kind);
        self.mod_installing = Some(project.clone());
        self.mod_error = None;
        self.mod_notice = None;
        let loader = loader.unwrap_or_default();
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        self.handle.spawn(async move {
            let result = match kind {
                ProjectType::Mod => {
                    modrinth::install_with_dependencies(
                        &project,
                        game_version.trim(),
                        &loader,
                        &target,
                    )
                    .await
                }
                _ => modrinth::install_content(&project, game_version.trim(), &target).await,
            }
            .map_err(|error| error.to_string());
            let _ = tx.send(Message::ModInstalled {
                dir: target,
                game_version,
                loader,
                kind,
                result,
            });
            ctx.request_repaint();
        });
    }

    fn install_page(&mut self, ui: &mut egui::Ui) {
        ui.heading("Install instance");
        let loader_key = (self.install_loader, self.install_version.trim().to_owned());
        if self.install_loader != LoaderChoice::Vanilla
            && self.loader_versions_key.as_ref() != Some(&loader_key)
            && !self.loader_versions_loading
        {
            self.loader_versions_key = Some(loader_key);
            self.reload_loader_versions(ui.ctx());
        }
        ui.horizontal(|ui| {
            ui.label("Version:");
            ui.add(
                egui::TextEdit::singleline(&mut self.install_version).id_salt("install_version"),
            );
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
                .id_salt("install_mc_versions")
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
            ui.label("Loader:");
            egui::ComboBox::from_id_salt("install_loader")
                .selected_text(self.install_loader.label())
                .show_ui(ui, |ui| {
                    for choice in LoaderChoice::ALL {
                        ui.selectable_value(&mut self.install_loader, choice, choice.label());
                    }
                });
            if self.install_loader != LoaderChoice::Vanilla {
                ui.label("Loader version:");
                ui.add(
                    egui::TextEdit::singleline(&mut self.install_loader_version)
                        .id_salt("install_loader_version")
                        .hint_text("latest"),
                );
                if ui
                    .add_enabled(!self.loader_versions_loading, egui::Button::new("Refresh"))
                    .clicked()
                {
                    self.loader_versions_key = Some((
                        self.install_loader,
                        self.install_version.trim().to_owned(),
                    ));
                    self.reload_loader_versions(ui.ctx());
                }
                if self.loader_versions_loading {
                    ui.add(egui::widgets::Spinner::new().size(14.0));
                }
            }
        });
        if self.install_loader != LoaderChoice::Vanilla {
            if let Some(error) = &self.loader_versions_error {
                ui.label(format!("loader list failed: {error}"));
            }
            if self.loader_versions.is_empty() {
                if !self.loader_versions_loading && self.loader_versions_error.is_none() {
                    ui.label("no compatible loader version found");
                }
            } else {
                egui::ScrollArea::vertical()
                    .id_salt("install_loader_versions")
                    .max_height(120.0)
                    .show(ui, |ui| {
                        for version in &self.loader_versions {
                            if ui
                                .selectable_label(
                                    self.install_loader_version == version.as_str(),
                                    version.as_str(),
                                )
                                .clicked()
                            {
                                self.install_loader_version = version.clone();
                            }
                        }
                    });
            }
            if matches!(
                self.install_loader,
                LoaderChoice::Forge | LoaderChoice::NeoForge
            ) {
                ui.horizontal(|ui| {
                    ui.label("Java:");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.java_path).id_salt("install_java"),
                    );
                    ui.label("used by the loader installer");
                });
            }
        }
        ui.horizontal(|ui| {
            ui.label("Minecraft dir:");
            ui.add(egui::TextEdit::singleline(&mut self.minecraft_dir).id_salt("install_dir"));
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
            ui.add(egui::TextEdit::singleline(&mut self.minecraft_dir).id_salt("launch_dir"));
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
            let java = self.effective_java(&selected);
            let isolate = self.effective_isolate(&selected);
            let source = if self.instance_uses_global(&selected) {
                "global"
            } else {
                "instance"
            };
            ui.label(
                egui::RichText::new(format!(
                    "Java: {}  ·  version isolation: {}  ({source} config, edit in Settings)",
                    java.display(),
                    if isolate { "on" } else { "off" }
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

    fn mods_page(&mut self, ui: &mut egui::Ui) {
        ui.heading("Search content");
        ui.label(egui::RichText::new("Powered by Modrinth").weak());
        let can_switch_type = self.mod_installing.is_none();
        ui.horizontal(|ui| {
            ui.label("Type:");
            ui.add_enabled_ui(can_switch_type, |ui| {
                for kind in [
                    ProjectType::Mod,
                    ProjectType::ResourcePack,
                    ProjectType::ShaderPack,
                    ProjectType::DataPack,
                ] {
                    if ui
                        .selectable_value(&mut self.mod_kind, kind, content_label(kind))
                        .changed()
                    {
                        self.mod_page = 1;
                        self.mod_results.clear();
                        self.mod_total = 0;
                        self.mod_updates.clear();
                        self.installed_mods.clear();
                        self.installed_mods_dir = String::new();
                        self.installed_kind = None;
                    }
                }
            });
        });
        ui.horizontal(|ui| {
            ui.label("Query:");
            let response = ui.add(
                egui::TextEdit::singleline(&mut self.mod_query)
                    .id_salt("mod_query")
                    .hint_text("e.g. sodium")
                    .desired_width(220.0),
            );
            let submitted =
                response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
            if ui
                .add_enabled(!self.mod_searching, egui::Button::new("Search"))
                .clicked()
                || submitted
            {
                self.start_mod_search(ui.ctx());
            }
            if self.mod_searching {
                ui.add(egui::widgets::Spinner::new().size(14.0));
            }
        });
        ui.horizontal(|ui| {
            ui.label("Minecraft:");
            ui.add(
                egui::TextEdit::singleline(&mut self.mod_minecraft_version)
                    .id_salt("mod_minecraft_version")
                    .hint_text("any")
                    .desired_width(100.0),
            );
            if self.mod_kind == ProjectType::Mod {
                ui.label("Loader:");
                egui::ComboBox::from_id_salt("mod_loader")
                    .selected_text(self.mod_loader.map(loader_label).unwrap_or("Any"))
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.mod_loader, None, "Any");
                        for kind in [
                            LoaderKind::Fabric,
                            LoaderKind::Quilt,
                            LoaderKind::Forge,
                            LoaderKind::NeoForge,
                        ] {
                            ui.selectable_value(
                                &mut self.mod_loader,
                                Some(kind),
                                loader_label(kind),
                            );
                        }
                    });
            }
            if self.mod_kind == ProjectType::DataPack {
                ui.label("World:");
                let selected = if self.mod_datapack_world.is_empty() {
                    "pick…".to_owned()
                } else {
                    self.mod_datapack_world.clone()
                };
                egui::ComboBox::from_id_salt("datapack_world")
                    .selected_text(selected)
                    .show_ui(ui, |ui| {
                        if self.datapack_worlds.is_empty() {
                            ui.label("no world found");
                        }
                        for world in &self.datapack_worlds {
                            ui.selectable_value(&mut self.mod_datapack_world, world.clone(), world);
                        }
                    });
            }
            if !self.installed.is_empty() {
                ui.label("From instance:");
                let mut picked: Option<(String, Option<String>, Option<&'static str>)> = None;
                egui::ComboBox::from_id_salt("mod_instance")
                    .selected_text(self.mod_instance_id.as_deref().unwrap_or("pick…"))
                    .show_ui(ui, |ui| {
                        for instance in &self.installed {
                            let label = match (instance.loader, &instance.game_version) {
                                (Some(tag), Some(version)) => {
                                    format!("{}  · {tag} {version}", instance.id)
                                }
                                (Some(tag), None) => format!("{}  · {tag}", instance.id),
                                (None, Some(version)) => {
                                    format!("{}  · {version}", instance.id)
                                }
                                (None, None) => instance.id.clone(),
                            };
                            if ui
                                .selectable_label(
                                    self.mod_instance_id.as_deref() == Some(instance.id.as_str()),
                                    label,
                                )
                                .clicked()
                            {
                                picked = Some((
                                    instance.id.clone(),
                                    instance.game_version.clone(),
                                    instance.loader,
                                ));
                            }
                        }
                    });
                if let Some((id, version, tag)) = picked {
                    self.mod_instance_id = Some(id);
                    if let Some(version) = version {
                        self.mod_minecraft_version = version;
                    }
                    if let Some(tag) = tag {
                        self.mod_loader = loader_kind(tag);
                    }
                }
            }
        });
        if let Some(error) = &self.mod_error {
            ui.label(format!("error: {error}"));
        }
        if let Some(notice) = &self.mod_notice {
            ui.label(notice);
        }
        let game_dir = self.game_dir();
        if self.mod_kind == ProjectType::DataPack
            && self.datapack_worlds_dir != game_dir.display().to_string()
        {
            self.scan_worlds(&game_dir);
        }
        let target_dir = self.content_dir(self.mod_kind);
        if self.installed_mods_dir != target_dir.display().to_string()
            || self.installed_kind != Some(self.mod_kind)
        {
            self.load_installed_mods(&target_dir, self.mod_kind);
            self.start_update_check(ui.ctx(), &target_dir, self.mod_kind);
        }
        let isolated_suffix = if self.mods_isolate() { " (isolated)" } else { "" };
        ui.label(
            egui::RichText::new(format!(
                "target: {}{isolated_suffix}",
                target_dir.display()
            ))
            .weak()
            .small(),
        );
        if self.mod_results.is_empty() {
            if !self.mod_searching && self.mod_error.is_none() {
                ui.label("enter a query, optionally filter by Minecraft version and loader");
            }
        } else {
            ui.horizontal(|ui| {
                ui.label(format!(
                    "{} results (showing {})",
                    self.mod_total,
                    self.mod_results.len()
                ));
                if ui.small_button("Reload icons").clicked() {
                    ui.ctx().forget_all_images();
                }
            });
            let target_ready = !self.mods_isolate() || self.mods_target_instance().is_some();
            let world_ready = self.mod_kind != ProjectType::DataPack
                || !self.mod_datapack_world.trim().is_empty();
            let needs_loader = self.mod_kind == ProjectType::Mod;
            let filters_ready = target_ready
                && world_ready
                && !self.mod_minecraft_version.trim().is_empty()
                && (!needs_loader || self.mod_loader.is_some());
            if !target_ready {
                ui.label(
                    egui::RichText::new(
                        "version isolation is on: pick an instance to install content into",
                    )
                    .weak()
                    .small(),
                );
            } else if !world_ready {
                ui.label(
                    egui::RichText::new("pick a world to install data packs into")
                        .weak()
                        .small(),
                );
            } else if !filters_ready {
                let hint = if needs_loader {
                    "set a Minecraft version and loader to install mods"
                } else {
                    "set a Minecraft version to install content"
                };
                ui.label(egui::RichText::new(hint).weak().small());
            }
            let mut pending: Option<(ModAction, String, Option<String>)> = None;
            // 给底部分页控件留出空间，避免滚动区占满高度把按钮挤出可视区域
            let pager_height = 40.0;
            let list_height = (ui.available_height() - pager_height).max(80.0);
            egui::ScrollArea::vertical()
                .id_salt("mod_results")
                .max_height(list_height)
                .show(ui, |ui| {
                    for hit in &self.mod_results {
                        let project_id = if hit.project_id.trim().is_empty() {
                            hit.slug.clone()
                        } else {
                            hit.project_id.clone()
                        };
                        let installed_key = if self.installed_mods.contains_key(&project_id) {
                            Some(project_id.clone())
                        } else if self.installed_mods.contains_key(&hit.slug) {
                            Some(hit.slug.clone())
                        } else {
                            None
                        };
                        let installed_version = installed_key
                            .as_ref()
                            .and_then(|key| self.installed_mods.get(key))
                            .map(|record| record.version_number.as_str());
                        let update_number = installed_key
                            .as_ref()
                            .and_then(|key| self.mod_updates.get(key))
                            .map(|(_, number)| number.as_str());
                        let installing_this =
                            self.mod_installing.as_deref() == Some(project_id.as_str());
                        let actions_enabled = self.mod_installing.is_none();
                        if let Some(action) = mod_result(
                            ui,
                            hit,
                            self.mod_kind,
                            target_ready,
                            filters_ready,
                            actions_enabled,
                            installing_this,
                            installed_version,
                            update_number,
                        ) {
                            pending = Some((action, project_id, installed_key));
                        }
                    }
                });
            if let Some((action, project_id, key)) = pending {
                match action {
                    ModAction::Uninstall => {
                        if let Some(key) = key {
                            self.uninstall_mod(&key, &target_dir, self.mod_kind);
                        }
                    }
                    ModAction::Install => {
                        let game_version = self.mod_minecraft_version.trim().to_owned();
                        let loader = if self.mod_kind == ProjectType::Mod {
                            self.mod_loader
                                .map(|loader| loader_slug(loader).to_owned())
                        } else {
                            None
                        };
                        self.start_mod_install(
                            project_id,
                            game_version,
                            loader,
                            self.mod_kind,
                            ui.ctx(),
                        );
                    }
                    ModAction::Reinstall | ModAction::Update => {
                        let record = key.and_then(|key| self.installed_mods.get(&key).cloned());
                        if let Some(record) = record {
                            // 旧清单没有记录版本与加载器，回退到当前筛选条件
                            let game_version = if record.game_version.trim().is_empty() {
                                self.mod_minecraft_version.trim().to_owned()
                            } else {
                                record.game_version.clone()
                            };
                            let loader = if self.mod_kind == ProjectType::Mod {
                                let loader = if record.loader.trim().is_empty() {
                                    self.mod_loader
                                        .map(|kind| loader_slug(kind).to_owned())
                                        .unwrap_or_default()
                                } else {
                                    record.loader.clone()
                                };
                                Some(loader)
                            } else {
                                None
                            };
                            self.start_mod_install(
                                project_id,
                                game_version,
                                loader,
                                self.mod_kind,
                                ui.ctx(),
                            );
                        }
                    }
                }
            }
            let page_size = u64::from(modrinth::PAGE_SIZE);
            let total_pages = self.mod_total.div_ceil(page_size).max(1) as u32;
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(
                        self.mod_page > 1 && !self.mod_searching,
                        egui::Button::new("Previous"),
                    )
                    .clicked()
                {
                    self.goto_mod_page(self.mod_page - 1, ui.ctx());
                }
                ui.label(format!("Page {} / {}", self.mod_page, total_pages));
                if ui
                    .add_enabled(
                        self.mod_page < total_pages && !self.mod_searching,
                        egui::Button::new("Next"),
                    )
                    .clicked()
                {
                    self.goto_mod_page(self.mod_page + 1, ui.ctx());
                }
            });
        }
    }

    fn settings_page(&mut self, ui: &mut egui::Ui) {
        ui.heading("Settings");
        let path = config::path()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|_| "unavailable".to_owned());
        ui.label(
            egui::RichText::new(format!("stored in {path}"))
                .weak()
                .small(),
        );
        egui::ScrollArea::vertical()
            .id_salt("settings")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.group(|ui| {
                    ui.label(egui::RichText::new("Global configuration").strong());
                    ui.horizontal(|ui| {
                        ui.label("Minecraft dir:");
                        ui.add(
                            egui::TextEdit::singleline(&mut self.minecraft_dir)
                                .id_salt("settings_minecraft_dir"),
                        );
                    });
                    ui.horizontal(|ui| {
                        ui.label("Java path:");
                        ui.add(
                            egui::TextEdit::singleline(&mut self.java_path)
                                .id_salt("settings_java")
                                .hint_text("auto-detect"),
                        );
                    });
                    ui.checkbox(&mut self.isolate, "Version isolation");
                });
                ui.add_space(10.0);
                ui.label(egui::RichText::new("Instances").strong());
                ui.label(
                    egui::RichText::new(
                        "Each instance uses the global configuration unless overridden below",
                    )
                    .weak()
                    .small(),
                );
                let ids: Vec<String> = self
                    .installed
                    .iter()
                    .map(|instance| instance.id.clone())
                    .collect();
                if ids.is_empty() {
                    ui.label(
                        egui::RichText::new("no installed instance found")
                            .weak()
                            .small(),
                    );
                }
                let global_isolate = self.isolate;
                for id in ids {
                    let mut use_global = self.instance_uses_global(&id);
                    let mut toggled = false;
                    ui.group(|ui| {
                        if ui
                            .checkbox(&mut use_global, format!("{id}: use global configuration"))
                            .changed()
                        {
                            toggled = true;
                        }
                        if use_global {
                            return;
                        }
                        let entry = self.config.instances.entry(id.clone()).or_insert_with(|| {
                            InstanceConfig {
                                use_global: false,
                                // 取消继承后从全局值起步，方便在此基础上修改
                                isolate: global_isolate,
                                java_path: String::new(),
                            }
                        });
                        ui.checkbox(&mut entry.isolate, "Version isolation");
                        ui.horizontal(|ui| {
                            ui.label("Java path:");
                            ui.add(
                                egui::TextEdit::singleline(&mut entry.java_path)
                                    .id_salt(("instance_java", id.as_str()))
                                    .hint_text("inherit global"),
                            );
                        });
                    });
                    if toggled {
                        let entry = self
                            .config
                            .instances
                            .entry(id.clone())
                            .or_insert_with(InstanceConfig::default);
                        if use_global {
                            entry.use_global = true;
                        } else if entry.use_global {
                            // 从全局配置切到单独配置时，以全局值作为起点
                            entry.use_global = false;
                            entry.isolate = global_isolate;
                            entry.java_path.clear();
                        }
                    }
                }
                ui.add_space(10.0);
                if ui.button("Save configuration").clicked() {
                    self.save_config();
                }
                if let Some(status) = &self.config_status {
                    ui.label(egui::RichText::new(status).weak().small());
                }
            });
    }
}

fn mod_result(
    ui: &mut egui::Ui,
    hit: &ModSearchHit,
    kind: ProjectType,
    target_ready: bool,
    filters_ready: bool,
    actions_enabled: bool,
    installing: bool,
    installed_version: Option<&str>,
    update_number: Option<&str>,
) -> Option<ModAction> {
    let mut action = None;
    let installed = installed_version.is_some();
    ui.group(|ui| {
        ui.horizontal_top(|ui| {
            if let Some(icon_url) = &hit.icon_url {
                ui.add(
                    egui::Image::new(icon_url.as_str()).fit_to_exact_size(egui::vec2(48.0, 48.0)),
                );
            }
            ui.vertical(|ui| {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(&hit.title).strong());
                    ui.label(egui::RichText::new(format!("{} downloads", hit.downloads)).weak());
                });
                if !hit.description.is_empty() {
                    ui.label(&hit.description);
                }
                let mut meta = hit.categories.join(", ");
                if !hit.versions.is_empty() {
                    if !meta.is_empty() {
                        meta.push_str(" · ");
                    }
                    meta.push_str(
                        &hit.versions
                            .iter()
                            .take(5)
                            .cloned()
                            .collect::<Vec<_>>()
                            .join(", "),
                    );
                }
                if !meta.is_empty() {
                    ui.label(egui::RichText::new(meta).weak().small());
                }
                if let Some(version) = installed_version {
                    let label = if version.is_empty() {
                        "installed".to_owned()
                    } else {
                        format!("installed v{version}")
                    };
                    ui.label(egui::RichText::new(label).weak().small());
                }
                ui.horizontal(|ui| {
                    if installing {
                        ui.add(egui::widgets::Spinner::new().size(12.0));
                    }
                    if installed {
                        if ui
                            .add_enabled(actions_enabled && target_ready, egui::Button::new("Uninstall"))
                            .clicked()
                        {
                            action = Some(ModAction::Uninstall);
                        }
                        if ui
                            .add_enabled(
                                actions_enabled && target_ready,
                                egui::Button::new("Reinstall"),
                            )
                            .clicked()
                        {
                            action = Some(ModAction::Reinstall);
                        }
                        if let Some(number) = update_number {
                            if ui
                                .add_enabled(
                                    actions_enabled && target_ready,
                                    egui::Button::new(format!("Update to {number}")),
                                )
                                .clicked()
                            {
                                action = Some(ModAction::Update);
                            }
                        }
                    } else if ui
                        .add_enabled(
                            actions_enabled && target_ready && filters_ready,
                            egui::Button::new("Install"),
                        )
                        .clicked()
                    {
                        action = Some(ModAction::Install);
                    }
                    ui.hyperlink_to(
                        "Open on Modrinth",
                        format!(
                            "https://modrinth.com/{}/{}",
                            kind.url_segment(),
                            hit.slug
                        ),
                    );
                });
            });
        });
    });
    ui.add_space(4.0);
    action
}

fn content_label(kind: ProjectType) -> &'static str {
    match kind {
        ProjectType::Mod => "Mods",
        ProjectType::ResourcePack => "Resource packs",
        ProjectType::ShaderPack => "Shader packs",
        ProjectType::DataPack => "Data packs",
    }
}

fn loader_label(kind: LoaderKind) -> &'static str {
    match kind {
        LoaderKind::Fabric => "Fabric",
        LoaderKind::Quilt => "Quilt",
        LoaderKind::Forge => "Forge",
        LoaderKind::NeoForge => "NeoForge",
    }
}

fn loader_slug(kind: LoaderKind) -> &'static str {
    match kind {
        LoaderKind::Fabric => "fabric",
        LoaderKind::Quilt => "quilt",
        LoaderKind::Forge => "forge",
        LoaderKind::NeoForge => "neoforge",
    }
}

async fn launch_task(
    auth: &Auth,
    minecraft_dir: &Path,
    version: &str,
    java: &Path,
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
            isolate,
        )
    }
}

fn spawn_game(
    minecraft_dir: &Path,
    version: &str,
    account: LaunchAccount<'_>,
    java: &Path,
    isolate: bool,
) -> Result<u32, String> {
    let child = launch::launch(LaunchRequest {
        minecraft_dir,
        version_id: version,
        account,
        java: Some(java),
        game_dir: None,
        isolated: isolate,
    })
    .map_err(|error| error.to_string())?;
    Ok(child.id())
}

fn loader_tag(version_id: &str) -> Option<&'static str> {
    if version_id.starts_with("fabric-loader-") {
        Some("Fabric")
    } else if version_id.starts_with("quilt-loader-") {
        Some("Quilt")
    } else if version_id.starts_with("neoforge-") {
        Some("NeoForge")
    } else if version_id.contains("-forge-") {
        Some("Forge")
    } else {
        None
    }
}

fn inspect_instance(minecraft_dir: &Path, version_id: &str) -> (Option<&'static str>, Option<String>) {
    let path = minecraft_dir
        .join("versions")
        .join(version_id)
        .join(format!("{version_id}.json"));
    let Ok(json) = std::fs::read_to_string(&path) else {
        return (loader_tag(version_id), None);
    };
    let loader = loader_tag(version_id).or_else(|| {
        if json.contains("net.fabricmc") {
            Some("Fabric")
        } else if json.contains("org.quiltmc") {
            Some("Quilt")
        } else if json.contains("net.neoforged") {
            Some("NeoForge")
        } else if json.contains("net.minecraftforge") {
            Some("Forge")
        } else {
            None
        }
    });
    let game_version = serde_json::from_str::<serde_json::Value>(&json)
        .ok()
        .and_then(|value| {
            value
                .get("inheritsFrom")
                .and_then(|version| version.as_str())
                .map(str::to_owned)
        })
        .or_else(|| Some(version_id.to_owned()));
    (loader, game_version)
}

fn loader_kind(tag: &str) -> Option<LoaderKind> {
    match tag {
        "Fabric" => Some(LoaderKind::Fabric),
        "Quilt" => Some(LoaderKind::Quilt),
        "Forge" => Some(LoaderKind::Forge),
        "NeoForge" => Some(LoaderKind::NeoForge),
        _ => None,
    }
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
                Message::LoaderVersions(result) => {
                    self.loader_versions_loading = false;
                    match result {
                        Ok(versions) => {
                            self.loader_versions = versions;
                            self.loader_versions_error = None;
                        }
                        Err(error) => self.loader_versions_error = Some(error),
                    }
                }
                Message::ModSearch { page, result } => {
                    self.mod_searching = false;
                    match result {
                        Ok(result) => {
                            self.mod_page = page;
                            self.mod_results = result.hits;
                            self.mod_total = result.total_hits;
                            self.mod_error = None;
                        }
                        Err(error) => self.mod_error = Some(error),
                    }
                }
                Message::ModInstalled {
                    dir,
                    game_version,
                    loader,
                    kind,
                    result,
                } => {
                    self.mod_installing = None;
                    match result {
                        Ok(outcome) => {
                            // 安装期间切换了类型或目录时，不再把记录写进当前视图
                            let same_target = self.mod_kind == kind
                                && self.installed_kind == Some(kind)
                                && self.installed_mods_dir == dir.display().to_string();
                            if same_target {
                                for (index, installed) in outcome.installed.iter().enumerate() {
                                    let old =
                                        self.installed_mods.get(&installed.project_id).cloned();
                                    // 旧清单用 slug 作键，重装后清掉指向同一个 jar 的旧记录
                                    let duplicate = self
                                        .installed_mods
                                        .iter()
                                        .find(|(key, record)| {
                                            key.as_str() != installed.project_id.as_str()
                                                && record.filename == installed.filename
                                        })
                                        .map(|(key, _)| key.clone());
                                    if let Some(duplicate) = duplicate {
                                        self.installed_mods.remove(&duplicate);
                                    }
                                    if let Some(old) = &old {
                                        if old.filename != installed.filename {
                                            let _ = std::fs::remove_file(dir.join(&old.filename));
                                        }
                                    }
                                    // 用户主动安装的项目不再算作自动前置
                                    let auto = if index == 0 {
                                        false
                                    } else {
                                        old.map(|record| record.auto).unwrap_or(true)
                                    };
                                    self.installed_mods.insert(
                                        installed.project_id.clone(),
                                        ModRecord {
                                            filename: installed.filename.clone(),
                                            version_id: installed.version_id.clone(),
                                            version_number: installed.version_number.clone(),
                                            game_version: game_version.clone(),
                                            loader: loader.clone(),
                                            auto,
                                            dependencies: installed.dependencies.clone(),
                                        },
                                    );
                                    self.mod_updates.remove(&installed.project_id);
                                }
                                self.save_installed_mods(&dir, kind);
                            }
                            let root_name = outcome
                                .root()
                                .map(|installed| installed.filename.as_str())
                                .unwrap_or("content");
                            let dependencies = outcome.installed.len().saturating_sub(1);
                            let mut notice =
                                format!("installed {root_name} into {}", dir.display());
                            if dependencies > 0 {
                                notice.push_str(&format!(" (+{dependencies} dependencies)"));
                            }
                            if !outcome.skipped.is_empty() {
                                notice.push_str(&format!(
                                    "; missing dependencies: {}",
                                    outcome.skipped.join(", ")
                                ));
                            }
                            self.mod_notice = Some(notice);
                        }
                        Err(error) => self.mod_error = Some(error),
                    }
                }
                Message::ModUpdates { dir, updates } => {
                    if dir.display().to_string() == self.installed_mods_dir {
                        self.mod_updates = updates
                            .into_iter()
                            .filter(|(key, (latest, _))| {
                                self.installed_mods
                                    .get(key)
                                    .map_or(false, |record| &record.version_id != latest)
                            })
                            .collect();
                    }
                }
            }
        }

        if !self.versions_requested {
            self.versions_requested = true;
            self.reload_versions(&ctx);
        }

        let current_dir = self.minecraft_dir.trim().to_owned();
        if current_dir != self.installed_dir {
            self.scan_installed();
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
            .resizable(false)
            .min_size(120.0)
            .max_size(120.0)
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
                ui.selectable_value(&mut self.page, Page::Mods, "Content");
                ui.selectable_value(&mut self.page, Page::Settings, "Settings");
            });
            ui.separator();
            match self.page {
                Page::Install => self.install_page(ui),
                Page::Launch => self.launch_page(ui),
                Page::Mods => self.mods_page(ui),
                Page::Settings => self.settings_page(ui),
            }
        });
    }
}
