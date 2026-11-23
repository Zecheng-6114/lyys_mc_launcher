use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, mpsc};

use eframe::egui;
use serde::{Deserialize, Serialize};
use tokio::runtime::Handle;

use mc_launcher_core::loader::LoaderKind;
use mc_launcher_core::loader::common::{LoaderSpec, LoaderVersion};

use crate::core::auth::{AccountSession, Auth};
use crate::core::config::LauncherConfig;
use crate::core::i18n::{self, Language, t, tf};
use crate::core::install::{self, AvailableVersion, InstallProgress};
use crate::core::java::{self, JavaRuntime};
use crate::core::modpack;
use crate::core::modrinth::{InstallOutcome, ModSearchHit, ModSearchResult, ProjectType};

use super::background::contour::paint_contour_background;

pub(crate) const DEFAULT_MINECRAFT_DIR: &str = ".minecraft";
/// Records which jar each installed mod slug maps to, so the UI can tell that a mod is installed.
pub(crate) const MOD_MANIFEST_FILE: &str = ".lyys_mods.json";

#[derive(Clone, Copy)]
pub(crate) enum Task {
    Login,
    Install,
    Launch,
}

pub(crate) enum Message {
    Log(String),
    TaskDone(Task),
    Progress(InstallProgress),
    Versions(Result<Vec<AvailableVersion>, String>),
    LoaderVersions {
        key: (LoaderChoice, String),
        result: Result<Vec<String>, String>,
    },
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
    JavaVersions(Vec<JavaRuntime>),
    ModpackProgress(String),
    ModpackDone(Result<modpack::InstallReport, String>),
}

#[derive(PartialEq, Eq, Clone, Copy)]
pub(crate) enum Page {
    Install,
    Launch,
    Mods,
    Settings,
}

#[derive(PartialEq, Eq, Clone, Copy)]
pub(crate) enum LoaderChoice {
    Vanilla,
    Fabric,
    Quilt,
    Forge,
    NeoForge,
}

impl LoaderChoice {
    pub(crate) const ALL: [Self; 5] = [
        Self::Vanilla,
        Self::Fabric,
        Self::Quilt,
        Self::Forge,
        Self::NeoForge,
    ];

    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Vanilla => "Vanilla",
            Self::Fabric => "Fabric",
            Self::Quilt => "Quilt",
            Self::Forge => "Forge",
            Self::NeoForge => "NeoForge",
        }
    }

    pub(crate) fn to_spec(self, version: &str) -> Option<LoaderSpec> {
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

    pub(crate) fn kind(self) -> Option<LoaderKind> {
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
pub(crate) struct InstalledInstance {
    pub(crate) id: String,
    pub(crate) loader: Option<&'static str>,
    pub(crate) game_version: Option<String>,
}

/// What the manifest remembers about one installed mod.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct ModRecord {
    pub(crate) filename: String,
    #[serde(default)]
    pub(crate) version_id: String,
    #[serde(default)]
    pub(crate) version_number: String,
    #[serde(default)]
    pub(crate) game_version: String,
    #[serde(default)]
    pub(crate) loader: String,
    /// True when the jar was pulled in as a required dependency.
    #[serde(default)]
    pub(crate) auto: bool,
    /// Project ids of the required dependencies.
    #[serde(default)]
    pub(crate) dependencies: Vec<String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ModAction {
    Install,
    Reinstall,
    Uninstall,
    Update,
}

pub(crate) struct App {
    pub(crate) auth: Arc<Auth>,
    pub(crate) handle: Handle,
    pub(crate) tx: mpsc::Sender<Message>,
    pub(crate) rx: mpsc::Receiver<Message>,
    pub(crate) accounts: Vec<AccountSession>,
    pub(crate) active: Option<String>,
    pub(crate) offline_name: String,
    pub(crate) page: Page,
    // 安装实例
    pub(crate) install_version: String,
    pub(crate) minecraft_dir: String,
    pub(crate) install_loader: LoaderChoice,
    pub(crate) install_loader_version: String,
    pub(crate) loader_versions: Vec<String>,
    pub(crate) loader_versions_loading: bool,
    pub(crate) loader_versions_error: Option<String>,
    pub(crate) loader_versions_key: Option<(LoaderChoice, String)>,
    pub(crate) java_path: String,
    /// Java runtimes from the last scan, shown in the settings page.
    pub(crate) java_versions: Vec<JavaRuntime>,
    pub(crate) java_scanning: bool,
    pub(crate) versions: Vec<AvailableVersion>,
    pub(crate) versions_loading: bool,
    pub(crate) versions_error: Option<String>,
    pub(crate) versions_requested: bool,
    pub(crate) show_all_versions: bool,
    pub(crate) install_progress: Option<InstallProgress>,
    pub(crate) installing: bool,
    // 启动实例
    pub(crate) launch_version: String,
    pub(crate) installed: Vec<InstalledInstance>,
    pub(crate) installed_dir: String,
    pub(crate) installed_error: Option<String>,
    pub(crate) launching: bool,
    pub(crate) isolate: bool,
    // 持久化配置
    pub(crate) config: LauncherConfig,
    pub(crate) config_status: Option<String>,
    // 搜索模组
    pub(crate) mod_query: String,
    pub(crate) mod_kind: ProjectType,
    pub(crate) mod_minecraft_version: String,
    pub(crate) mod_loader: Option<LoaderKind>,
    pub(crate) mod_instance_id: Option<String>,
    pub(crate) mod_datapack_world: String,
    pub(crate) datapack_worlds: Vec<String>,
    pub(crate) datapack_worlds_dir: String,
    pub(crate) mod_results: Vec<ModSearchHit>,
    pub(crate) mod_total: u64,
    /// 当前页码，从 1 开始。
    pub(crate) mod_page: u32,
    pub(crate) mod_searching: bool,
    pub(crate) mod_installing: Option<String>,
    pub(crate) mod_notice: Option<String>,
    pub(crate) mod_error: Option<String>,
    pub(crate) installed_mods: HashMap<String, ModRecord>,
    pub(crate) installed_mods_dir: String,
    pub(crate) installed_kind: Option<ProjectType>,
    pub(crate) mod_updates: HashMap<String, (String, String)>,
    // 安装整合包
    pub(crate) modpack_file: String,
    pub(crate) modpack_with_optional: bool,
    pub(crate) modpack_installing: bool,
    pub(crate) modpack_progress: Option<String>,
    pub(crate) modpack_notice: Option<String>,
    pub(crate) modpack_error: Option<String>,
    pub(crate) logging_in: bool,
    pub(crate) log: Vec<String>,
}

impl App {
    fn new(auth: Auth, handle: Handle) -> Self {
        let (tx, rx) = mpsc::channel();
        let (config, load_error) = match LauncherConfig::load() {
            Ok(config) => (config, None),
            Err(error) => (LauncherConfig::default(), Some(error)),
        };
        let language = Language::from_code(&config.language).unwrap_or_else(Language::detect);
        i18n::set_language(language);
        let config_status = load_error.map(|error| {
            let error = error.to_string();
            tf("failed to load configuration: {}", &[error.as_str()])
        });
        let minecraft_dir = if config.minecraft_dir.trim().is_empty() {
            DEFAULT_MINECRAFT_DIR.to_owned()
        } else {
            config.minecraft_dir.trim().to_owned()
        };
        // 留空表示启动时按游戏版本自动匹配本机 Java
        let java_path = config.java_path.trim().to_owned();
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
            java_versions: Vec::new(),
            java_scanning: false,
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
            modpack_file: String::new(),
            modpack_with_optional: false,
            modpack_installing: false,
            modpack_progress: None,
            modpack_notice: None,
            modpack_error: None,
            logging_in: false,
            log: Vec::new(),
        };
        // 先用上次缓存的版本列表，避免每次启动都要等网络
        if let Some(versions) = install::cached_versions() {
            if let Some(latest) = versions.iter().find(|entry| entry.kind == "release") {
                app.install_version = latest.id.clone();
            }
            app.versions = versions;
            app.versions_requested = true;
        }
        app.reload_accounts();
        app.scan_installed();
        app
    }

    pub(crate) fn reload_accounts(&mut self) {
        match (self.auth.list(), self.auth.active_uuid()) {
            (Ok(accounts), Ok(active)) => {
                self.accounts = accounts;
                self.active = active;
            }
            (Err(error), _) | (_, Err(error)) => {
                let error = error.to_string();
                self.log
                    .push(tf("failed to read accounts: {}", &[error.as_str()]));
            }
        }
    }

    pub(crate) fn scan_installed(&mut self) {
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

    /// Whether `instance` falls back to the global configuration.
    pub(crate) fn instance_uses_global(&self, instance: &str) -> bool {
        self.config
            .instances
            .get(instance)
            .map(|config| config.use_global)
            .unwrap_or(true)
    }

    /// Version isolation for `instance`: its own config, else the global toggle.
    pub(crate) fn effective_isolate(&self, instance: &str) -> bool {
        match self.config.instances.get(instance) {
            Some(config) if !config.use_global => config.isolate,
            _ => self.isolate,
        }
    }

    /// Java executable for `instance`: its own config, else the global one.
    /// `None` means no Java was configured and one is matched at launch time.
    pub(crate) fn effective_java(&self, instance: &str) -> Option<PathBuf> {
        let configured = match self.config.instances.get(instance) {
            Some(config) if !config.use_global => config.java_path.trim(),
            _ => "",
        };
        let path = if configured.is_empty() {
            self.java_path.trim()
        } else {
            configured
        };
        (!path.is_empty()).then(|| PathBuf::from(path))
    }

    /// Java executables the launcher knows about: the saved list plus this scan.
    pub(crate) fn java_candidates(&self) -> Vec<String> {
        let mut candidates = self.config.java_versions.clone();
        for runtime in &self.java_versions {
            let path = runtime.executable.display().to_string();
            if !candidates.contains(&path) {
                candidates.push(path);
            }
        }
        let selected = self.java_path.trim();
        if !selected.is_empty() && !candidates.iter().any(|path| path == selected) {
            candidates.push(selected.to_owned());
        }
        candidates
    }

    /// Scan this machine for Java runtimes on a background thread.
    pub(crate) fn scan_java(&mut self, ctx: egui::Context) {
        if self.java_scanning {
            return;
        }
        self.java_scanning = true;
        let dir = PathBuf::from(self.minecraft_dir.trim());
        let known = self.java_candidates();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let runtimes = java::discover(&dir, &known);
            let _ = tx.send(Message::JavaVersions(runtimes));
            ctx.request_repaint();
        });
    }

    /// Persist the configuration, syncing the runtime global values into it.
    pub(crate) fn save_config(&mut self) {
        self.config.minecraft_dir = self.minecraft_dir.trim().to_owned();
        self.config.java_path = self.java_path.trim().to_owned();
        self.config.java_versions = self.java_candidates();
        self.config.isolate = self.isolate;
        self.config.language = i18n::current().code().to_owned();
        match self.config.save() {
            Ok(path) => {
                let path = path.display().to_string();
                self.config_status = Some(tf("saved to {}", &[path.as_str()]));
                self.scan_installed();
            }
            Err(error) => {
                let error = error.to_string();
                self.config_status = Some(tf("save failed: {}", &[error.as_str()]));
            }
        }
    }

    /// Save the Java choice (radio selection or scan results) right away,
    /// leaving other unsaved settings untouched.
    pub(crate) fn persist_java(&mut self) {
        self.config.java_path = self.java_path.trim().to_owned();
        self.config.java_versions = self.java_candidates();
        match self.config.save() {
            Ok(path) => {
                let path = path.display().to_string();
                self.config_status = Some(tf("saved to {}", &[path.as_str()]));
            }
            Err(error) => {
                let error = error.to_string();
                self.config_status = Some(tf("save failed: {}", &[error.as_str()]));
            }
        }
    }

    pub(crate) fn start_login(&mut self, ctx: &egui::Context) {
        self.logging_in = true;
        let auth = Arc::clone(&self.auth);
        let tx = self.tx.clone();
        let prompt_tx = tx.clone();
        let ctx = ctx.clone();
        self.handle.spawn(async move {
            let result = auth
                .login(move |prompt| {
                    let _ = prompt_tx.send(Message::Log(tf(
                        "open {} and enter code {}",
                        &[prompt.verification_uri.as_str(), prompt.user_code.as_str()],
                    )));
                })
                .await;
            let line = match result {
                Ok(session) => tf("logged in: {}", &[session.profile.name.as_str()]),
                Err(error) => {
                    let error = error.to_string();
                    tf("login failed: {}", &[error.as_str()])
                }
            };
            let _ = tx.send(Message::Log(line));
            let _ = tx.send(Message::TaskDone(Task::Login));
            ctx.request_repaint();
        });
    }
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
                Message::ModpackProgress(line) => self.modpack_progress = Some(line),
                Message::ModpackDone(result) => {
                    self.modpack_installing = false;
                    self.modpack_progress = None;
                    self.mod_installing = None;
                    match result {
                        Ok(report) => {
                            let version = report.version_id.clone();
                            self.modpack_notice =
                                Some(tf("modpack installed: {}", &[version.as_str()]));
                            self.log.push(tf("modpack installed: {}", &[version.as_str()]));
                            self.log.push(report.timings.summary());
                            self.launch_version = version;
                            self.scan_installed();
                        }
                        Err(error) => {
                            let line =
                                tf("modpack install failed: {}", &[error.as_str()]);
                            self.modpack_error = Some(line.clone());
                            self.log.push(line);
                        }
                    }
                }
                Message::Versions(result) => {
                    self.versions_loading = false;
                    match result {
                        Ok(versions) => {
                            install::store_versions(&versions);
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
                Message::LoaderVersions { key, result } => match result {
                    Ok(versions) => {
                        if let Some(kind) = key.0.kind() {
                            install::store_loader_versions(kind, &key.1, &versions);
                        }
                        if self.loader_versions_key.as_ref() == Some(&key) {
                            self.loader_versions = versions;
                            self.loader_versions_error = None;
                            self.loader_versions_loading = false;
                        }
                    }
                    Err(error) => {
                        if self.loader_versions_key.as_ref() == Some(&key) {
                            self.loader_versions_error = Some(error);
                            self.loader_versions_loading = false;
                        }
                    }
                },
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
                                .unwrap_or_else(|| t("content"));
                            let dependencies = outcome.installed.len().saturating_sub(1);
                            let directory = dir.display().to_string();
                            let mut notice =
                                tf("installed {} into {}", &[root_name, directory.as_str()]);
                            if dependencies > 0 {
                                let dependencies = dependencies.to_string();
                                notice.push_str(&tf(
                                    " (+{} dependencies)",
                                    &[dependencies.as_str()],
                                ));
                            }
                            if !outcome.skipped.is_empty() {
                                let skipped = outcome.skipped.join(", ");
                                notice.push_str(&tf(
                                    "; missing dependencies: {}",
                                    &[skipped.as_str()],
                                ));
                            }
                            self.mod_notice = Some(notice);
                        }
                        Err(error) => self.mod_error = Some(error),
                    }
                }
                Message::JavaVersions(runtimes) => {
                    self.java_versions = runtimes;
                    self.java_scanning = false;
                    self.persist_java();
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

        self.accounts_panel(ui);

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
            self.page_tabs(ui);
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
