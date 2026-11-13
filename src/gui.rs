use std::collections::VecDeque;
use std::error::Error;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, App, AppContext, Bounds, Context, CursorStyle, Div, ElementId, Entity, IntoElement,
    ParentElement, Render, ScrollHandle, SharedString, Stateful, Styled, Window, WindowBounds,
    WindowOptions, div, prelude::*, px, size,
};
use gpui_component::{
    ActiveTheme, Icon, IconName, Root, Sizable, StyledExt, TitleBar, h_flex, input,
    switch::Switch,
    theme::{Theme, ThemeMode},
    v_flex,
};
use gpui_kit_assets::Assets;
use gpui_platform::application;
use serde_json::{Value, json};
use smol::channel;

use crate::i18n::{self, Lang};
use crate::instance::{self, LaunchConfig};

mod skin {
    use gpui::{Rgba, rgb};

    pub const WINDOW: u32 = 0x0f0f0f;
    pub const TITLEBAR_LINE: u32 = 0x1f1f1f;
    pub const PANEL: u32 = 0x242424;
    pub const SIDEBAR_TEXT: u32 = 0xd0d0d0;
    pub const CONTENT: u32 = 0x161616;
    pub const CARD: u32 = 0x242424;
    pub const DIVIDER: u32 = 0x2f2f2f;
    pub const HOVER: u32 = 0x2e2e2e;
    pub const ACTIVE: u32 = 0x3a3a3a;
    pub const FIELD: u32 = 0x2f2f2f;
    pub const FIELD_BORDER: u32 = 0x3f3f3f;
    pub const TEXT: u32 = 0xfafafa;
    pub const MUTED: u32 = 0xa3a3a3;
    pub const FAINT: u32 = 0x757575;
    pub const ACCENT: u32 = 0xfafafa;
    pub const ACCENT_HOVER: u32 = 0xffffff;
    pub const ACCENT_ACTIVE: u32 = 0xe4e4e4;
    pub const ACCENT_FG: u32 = 0x121212;
    pub const BUSY: u32 = 0x7aa2f7;
    pub const OK: u32 = 0x4ade80;
    pub const DANGER: u32 = 0x7f1d1d;
    pub const DANGER_HOVER: u32 = 0x991b1b;
    pub const DANGER_FG: u32 = 0xfecaca;

    pub fn c(hex: u32) -> Rgba {
        rgb(hex)
    }
}

fn apply_skin(cx: &mut App) {
    Theme::change(ThemeMode::Dark, None, cx);
    Theme::update(cx, |theme| {
        theme.radius = px(8.);
        theme.radius_lg = px(12.);
        theme.focus_ring = false;
        let colors = &mut theme.colors;
        colors.background = skin::c(skin::CONTENT).into();
        colors.foreground = skin::c(skin::TEXT).into();
        colors.border = skin::c(skin::DIVIDER).into();
        colors.muted = skin::c(skin::FIELD).into();
        colors.muted_foreground = skin::c(skin::MUTED).into();
        colors.accent = skin::c(skin::ACTIVE).into();
        colors.accent_foreground = skin::c(skin::TEXT).into();
        colors.title_bar = skin::c(skin::WINDOW).into();
        colors.sidebar = skin::c(skin::PANEL).into();
        colors.sidebar_foreground = skin::c(skin::SIDEBAR_TEXT).into();
        colors.sidebar_accent = skin::c(skin::ACTIVE).into();
        colors.sidebar_accent_foreground = skin::c(skin::TEXT).into();
        colors.sidebar_border = skin::c(skin::PANEL).into();
        colors.sidebar_primary = skin::c(skin::ACCENT).into();
        colors.sidebar_primary_foreground = skin::c(skin::ACCENT_FG).into();
        colors.list = skin::c(skin::CARD).into();
        colors.list_hover = skin::c(skin::HOVER).into();
        colors.list_active = skin::c(skin::ACTIVE).into();
        colors.list_active_border = skin::c(skin::DIVIDER).into();
        colors.list_head = skin::c(skin::CARD).into();
        colors.input = skin::c(skin::FIELD_BORDER).into();
        colors.popover = skin::c(skin::CARD).into();
        colors.popover_foreground = skin::c(skin::TEXT).into();
        colors.primary = skin::c(skin::ACCENT).into();
        colors.primary_hover = skin::c(skin::ACCENT_HOVER).into();
        colors.primary_active = skin::c(skin::ACCENT_ACTIVE).into();
        colors.primary_foreground = skin::c(skin::ACCENT_FG).into();
        colors.button = skin::c(skin::FIELD).into();
        colors.button_hover = skin::c(skin::HOVER).into();
        colors.button_active = skin::c(skin::ACTIVE).into();
        colors.button_foreground = skin::c(skin::TEXT).into();
        colors.button_primary = skin::c(skin::ACCENT).into();
        colors.button_primary_hover = skin::c(skin::ACCENT_HOVER).into();
        colors.button_primary_active = skin::c(skin::ACCENT_ACTIVE).into();
        colors.button_primary_foreground = skin::c(skin::ACCENT_FG).into();
        colors.button_secondary = skin::c(skin::FIELD).into();
        colors.button_secondary_hover = skin::c(skin::HOVER).into();
        colors.button_secondary_foreground = skin::c(skin::TEXT).into();
        colors.ring = skin::c(skin::MUTED).into();
        colors.selection = skin::c(skin::ACTIVE).into();
        colors.caret = skin::c(skin::TEXT).into();
        colors.scrollbar_thumb = skin::c(skin::ACTIVE).into();
        colors.scrollbar_thumb_hover = skin::c(skin::MUTED).into();
        colors.switch = skin::c(0x424242).into();
        colors.switch_thumb = skin::c(0xe8e8e8).into();
    });
}

struct TaskHandle {
    child: Mutex<Option<std::process::Child>>,
    cancelled: AtomicBool,
}

enum TaskEvent {
    Log(String),
    Progress { done: usize, total: usize },
    Done(bool),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TaskState {
    Running,
    Success,
    Failed,
    Cancelled,
}

fn task_state_key(state: TaskState) -> &'static str {
    match state {
        TaskState::Running => "task.state.running",
        TaskState::Success => "task.state.success",
        TaskState::Failed => "task.state.failed",
        TaskState::Cancelled => "task.state.cancelled",
    }
}

struct TaskEntry {
    id: usize,
    title: SharedString,
    instance: Option<SharedString>,
    state: TaskState,
    progress: Option<f32>,
    log_lines: VecDeque<SharedString>,
    last_log_notify: Instant,
    handle: Arc<TaskHandle>,
}

#[derive(Clone)]
struct InstanceRow {
    name: SharedString,
    version: SharedString,
    loader: SharedString,
    isolated: bool,
}

pub struct LauncherUi {
    tab: usize,
    instance_page: usize,
    download_tab: usize,
    lang: Lang,
    language: Option<SharedString>,
    instances: Vec<InstanceRow>,
    tasks: Vec<TaskEntry>,
    active_task: Option<usize>,
    next_task_id: usize,
    task_log_scroll: ScrollHandle,
    task_log_follow: bool,

    dir_input: Entity<input::InputState>,
    username_input: Entity<input::InputState>,
    memory_input: Entity<input::InputState>,
    java_input: Entity<input::InputState>,
    mirror_input: Entity<input::InputState>,
    maven_input: Entity<input::InputState>,
    isolate: bool,

    install_version_input: Entity<input::InputState>,
    install_name_input: Entity<input::InputState>,
    install_loader_input: Entity<input::InputState>,
    install_loader_version_input: Entity<input::InputState>,
    install_username_input: Entity<input::InputState>,
    install_memory_input: Entity<input::InputState>,
    install_java_input: Entity<input::InputState>,
    install_mirror_input: Entity<input::InputState>,
    install_maven_input: Entity<input::InputState>,
    install_isolate: bool,

    cfg_name: SharedString,
    cfg_username_input: Entity<input::InputState>,
    cfg_memory_input: Entity<input::InputState>,
    cfg_java_input: Entity<input::InputState>,
    cfg_mirror_input: Entity<input::InputState>,
    cfg_maven_input: Entity<input::InputState>,
    cfg_isolate: bool,

    import_file_input: Entity<input::InputState>,
    import_name_input: Entity<input::InputState>,
    import_optional: bool,

    mod_query_input: Entity<input::InputState>,
    mod_loader_input: Entity<input::InputState>,
    mod_version_input: Entity<input::InputState>,
    mod_limit_input: Entity<input::InputState>,
    mod_slug_input: Entity<input::InputState>,
    mod_instance_input: Entity<input::InputState>,
}

pub fn run() -> Result<(), Box<dyn Error>> {
    application().with_assets(Assets).run(|cx: &mut App| {
        gpui_component::init(cx);
        apply_skin(cx);

        let bounds = Bounds::centered(None, size(px(1180.), px(800.)), cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            window_min_size: Some(size(px(980.), px(640.))),
            ..TitleBar::window_options()
        };
        cx.open_window(options, |window, cx| {
            let view = cx.new(|cx| LauncherUi::new(window, cx));
            cx.new(|cx| Root::new(view, window, cx))
        })
        .unwrap();
        let on_closed = cx.on_window_closed(|cx, _window_id| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        });
        on_closed.detach();
    });
    Ok(())
}

impl LauncherUi {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let settings = load_settings();
        let memory = settings.memory.unwrap_or(4096).min(65536).to_string();
        let lang = settings
            .language
            .as_deref()
            .and_then(Lang::from_tag)
            .unwrap_or(i18n::FALLBACK);

        let mut this = LauncherUi {
            tab: 0,
            instance_page: 0,
            download_tab: 0,
            lang,
            language: settings.language.clone().map(SharedString::from),
            instances: Vec::new(),
            tasks: Vec::new(),
            active_task: None,
            next_task_id: 1,
            task_log_scroll: ScrollHandle::new(),
            task_log_follow: false,

            dir_input: input(window, &settings.game_dir, cx),
            username_input: input(window, &settings.username, cx),
            memory_input: input(window, &memory, cx),
            java_input: input(window, &settings.java, cx),
            mirror_input: input(window, &settings.mirror, cx),
            maven_input: input(window, &settings.maven, cx),
            isolate: settings.isolate,

            install_version_input: input(window, "1.20.1", cx),
            install_name_input: input(window, "", cx),
            install_loader_input: input(window, "", cx),
            install_loader_version_input: input(window, "", cx),
            install_username_input: input(window, &settings.username, cx),
            install_memory_input: input(window, &memory.clone(), cx),
            install_java_input: input(window, &settings.java.clone(), cx),
            install_mirror_input: input(window, &settings.mirror.clone(), cx),
            install_maven_input: input(window, &settings.maven.clone(), cx),
            install_isolate: settings.isolate,

            cfg_name: SharedString::default(),
            cfg_username_input: input(window, "", cx),
            cfg_memory_input: input(window, &memory.clone(), cx),
            cfg_java_input: input(window, &settings.java.clone(), cx),
            cfg_mirror_input: input(window, &settings.mirror.clone(), cx),
            cfg_maven_input: input(window, &settings.maven.clone(), cx),
            cfg_isolate: false,

            import_file_input: input(window, "", cx),
            import_name_input: input(window, "", cx),
            import_optional: false,

            mod_query_input: input(window, "", cx),
            mod_loader_input: input(window, "", cx),
            mod_version_input: input(window, "", cx),
            mod_limit_input: input(window, "10", cx),
            mod_slug_input: input(window, "", cx),
            mod_instance_input: input(window, "", cx),
        };
        this.reload(cx);
        this
    }

    fn value(entity: &Entity<input::InputState>, cx: &Context<Self>) -> String {
        entity.read(cx).value().trim().to_string()
    }

    fn set_input(
        entity: &Entity<input::InputState>,
        value: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let value = value.to_string();
        entity.update(cx, move |state, cx| state.set_value(value, window, cx));
    }

    fn memory_value(&self, entity: &Entity<input::InputState>, cx: &Context<Self>) -> Option<u32> {
        let text = Self::value(entity, cx);
        if text.is_empty() {
            return None;
        }
        text.parse().ok()
    }

    fn t(&self, key: &str) -> SharedString {
        i18n::text(self.lang, key)
    }

    fn t_args(&self, key: &str, args: &[(&str, &str)]) -> SharedString {
        i18n::text_args(self.lang, key, args)
    }

    fn select_language(&mut self, lang: Lang, cx: &mut Context<Self>) {
        if self.lang == lang {
            return;
        }
        self.lang = lang;
        self.language = Some(SharedString::from(lang.tag()));
        let value = self.settings_value(cx);
        let _ = write_json(&config_path(), &value, lang);
        cx.notify();
    }

    fn settings_value(&self, cx: &Context<Self>) -> Value {
        let memory = self.memory_value(&self.memory_input, cx);
        let mut value = json!({
            "game_dir": Self::value(&self.dir_input, cx),
            "username": Self::value(&self.username_input, cx),
            "memory": memory.unwrap_or(4096),
            "java": Self::value(&self.java_input, cx),
            "mirror": Self::value(&self.mirror_input, cx),
            "maven": Self::value(&self.maven_input, cx),
            "isolate": self.isolate,
        });
        if let Some(language) = &self.language {
            value["language"] = json!(language.to_string());
        }
        value
    }

    fn reload(&mut self, cx: &mut Context<Self>) {
        let dir = Self::value(&self.dir_input, cx);
        self.instances = list_instances(&dir);
        cx.notify();
    }

    fn push_task_log(&mut self, id: usize, line: String, cx: &mut Context<Self>) {
        let Some(task) = self.tasks.iter_mut().find(|task| task.id == id) else {
            return;
        };
        task.log_lines.push_back(SharedString::from(line));
        while task.log_lines.len() > 2_000 {
            task.log_lines.pop_front();
        }
        let now = Instant::now();
        if now.duration_since(task.last_log_notify) >= Duration::from_millis(50) {
            task.last_log_notify = now;
            self.task_log_follow = true;
            cx.notify();
        }
    }

    fn running_task_for(&self, name: &str) -> Option<usize> {
        self.tasks
            .iter()
            .find(|task| {
                task.state == TaskState::Running && task.instance.as_deref() == Some(name)
            })
            .map(|task| task.id)
    }

    fn start_task(
        &mut self,
        title: SharedString,
        instance: Option<SharedString>,
        args: Vec<String>,
        refresh: bool,
        save: Option<(String, String, LaunchConfig)>,
        cx: &mut Context<Self>,
    ) {
        let exe = match std::env::current_exe() {
            Ok(path) => path,
            Err(_) => return,
        };
        let id = self.next_task_id;
        self.next_task_id += 1;
        let handle = Arc::new(TaskHandle {
            child: Mutex::new(None),
            cancelled: AtomicBool::new(false),
        });
        self.tasks.push(TaskEntry {
            id,
            title,
            instance,
            state: TaskState::Running,
            progress: None,
            log_lines: VecDeque::new(),
            last_log_notify: Instant::now(),
            handle: handle.clone(),
        });
        if self.active_task.is_none() {
            self.active_task = Some(id);
        }
        self.task_log_follow = true;
        cx.notify();

        let (event_tx, event_rx) = channel::unbounded::<TaskEvent>();
        let lang = self.lang;
        std::thread::spawn(move || {
            execute(exe, args, event_tx, handle, lang);
        });

        cx.spawn(async move |weak, cx| {
            let mut success = false;
            while let Ok(event) = event_rx.recv().await {
                let stop = match event {
                    TaskEvent::Log(line) => {
                        weak.update(cx, |this, cx| this.push_task_log(id, line, cx))
                            .ok();
                        false
                    }
                    TaskEvent::Progress { done, total } => {
                        weak.update(cx, |this, cx| {
                            let value = if total == 0 {
                                0.0
                            } else {
                                done as f32 / total as f32
                            };
                            if let Some(task) = this.tasks.iter_mut().find(|task| task.id == id) {
                                task.progress = Some(value.clamp(0.0, 1.0));
                            }
                            cx.notify();
                        })
                        .ok();
                        false
                    }
                    TaskEvent::Done(ok) => {
                        success = ok;
                        true
                    }
                };
                if stop {
                    break;
                }
            }
            weak.update(cx, |this, cx| {
                let cancelled = this
                    .tasks
                    .iter()
                    .find(|task| task.id == id)
                    .map(|task| task.handle.cancelled.load(Ordering::SeqCst))
                    .unwrap_or(false);
                if let Some(task) = this.tasks.iter_mut().find(|task| task.id == id) {
                    task.state = if cancelled {
                        TaskState::Cancelled
                    } else if success {
                        TaskState::Success
                    } else {
                        TaskState::Failed
                    };
                    task.progress = None;
                }
                if success
                    && let Some((dir, name, config)) = save
                    && let Err(error) = instance::write_launch_config(&dir, &name, &config)
                {
                    let text = this
                        .t_args("error.save_instance", &[("error", &error.to_string())])
                        .to_string();
                    this.push_task_log(id, text, cx);
                }
                if refresh {
                    this.reload(cx);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn cancel_task(&mut self, id: usize, cx: &mut Context<Self>) {
        let Some(task) = self.tasks.iter().find(|task| task.id == id) else {
            return;
        };
        if task.state != TaskState::Running {
            return;
        }
        task.handle.cancelled.store(true, Ordering::SeqCst);
        if let Some(child) = task.handle.child.lock().unwrap().take() {
            std::thread::spawn(move || kill_tree(child));
        }
        cx.notify();
    }

    fn effective_launch(&self, name: &str, cx: &Context<Self>) -> (String, LaunchParams) {
        let dir = Self::value(&self.dir_input, cx);
        let config = instance::read_launch_config(&dir, name);
        let params = LaunchParams {
            username: config
                .username
                .unwrap_or_else(|| Self::value(&self.username_input, cx)),
            memory: config
                .memory
                .or_else(|| self.memory_value(&self.memory_input, cx)),
            java: config
                .java
                .unwrap_or_else(|| Self::value(&self.java_input, cx)),
            mirror: config
                .mirror
                .unwrap_or_else(|| Self::value(&self.mirror_input, cx)),
            maven: config
                .maven
                .unwrap_or_else(|| Self::value(&self.maven_input, cx)),
            isolate: config.isolate.unwrap_or(self.isolate),
        };
        (dir, params)
    }

    fn launch_instance(&mut self, name: &str, cx: &mut Context<Self>) {
        if let Some(id) = self.running_task_for(name) {
            self.active_task = Some(id);
            cx.notify();
            return;
        }
        let (dir, params) = self.effective_launch(name, cx);
        if dir.is_empty() {
            cx.notify();
            return;
        }
        let mut args = vec![
            String::from("launch"),
            String::from("--dir"),
            dir,
            String::from("--name"),
            name.to_string(),
            String::from("--username"),
            params.username,
            String::from("--memory"),
            params.memory.unwrap_or(4096).to_string(),
        ];
        if params.isolate {
            args.push(String::from("--isolate"));
        }
        push_option(&mut args, "--java", &params.java);
        push_option(&mut args, "--mirror", &params.mirror);
        push_option(&mut args, "--maven", &params.maven);

        let title = self.t_args("task.title.launch", &[("name", name)]);
        self.start_task(
            title,
            Some(SharedString::from(name)),
            args,
            false,
            None,
            cx,
        );
    }

    fn open_folder(&mut self, name: &str, cx: &mut Context<Self>) {
        let dir = Self::value(&self.dir_input, cx);
        let path = Path::new(&dir).join("versions").join(name);
        if !path.is_dir() {
            cx.notify();
            return;
        }
        let _ = Command::new("explorer").arg(path).spawn();
        cx.notify();
    }

    fn back_to_instances(&mut self, cx: &mut Context<Self>) {
        self.instance_page = 0;
        cx.notify();
    }

    fn install_version(&mut self, cx: &mut Context<Self>) {
        let dir = Self::value(&self.dir_input, cx);
        if dir.is_empty() {
            return;
        }
        let version = Self::value(&self.install_version_input, cx);
        if version.is_empty() {
            return;
        }
        let name = Self::value(&self.install_name_input, cx);
        let loader = Self::value(&self.install_loader_input, cx);
        let loader_version = Self::value(&self.install_loader_version_input, cx);
        let username = Self::value(&self.install_username_input, cx);
        let java = Self::value(&self.install_java_input, cx);
        let mirror = Self::value(&self.install_mirror_input, cx);
        let maven = Self::value(&self.install_maven_input, cx);
        let memory = self.memory_value(&self.install_memory_input, cx);

        let mut args = vec![
            String::from("download"),
            String::from("--dir"),
            dir.clone(),
            String::from("--version"),
            version.clone(),
        ];
        push_option(&mut args, "--name", &name);
        push_option(&mut args, "--loader", &loader);
        push_option(&mut args, "--loader-version", &loader_version);
        push_option(&mut args, "--java", &java);
        push_option(&mut args, "--mirror", &mirror);
        push_option(&mut args, "--maven", &maven);

        let instance = (!name.is_empty()).then(|| SharedString::from(name.clone()));
        let save = if name.is_empty() {
            None
        } else {
            Some((
                dir,
                name,
                LaunchConfig {
                    username: Some(username),
                    memory,
                    java: Some(java),
                    mirror: Some(mirror),
                    maven: Some(maven),
                    isolate: Some(self.install_isolate),
                },
            ))
        };
        let title = self.t_args("task.title.install", &[("version", &version)]);
        self.start_task(
            title,
            instance,
            args,
            true,
            save,
            cx,
        );
    }

    fn import_pack(&mut self, cx: &mut Context<Self>) {
        let dir = Self::value(&self.dir_input, cx);
        if dir.is_empty() {
            return;
        }
        let file = Self::value(&self.import_file_input, cx);
        if file.is_empty() {
            return;
        }
        let name = Self::value(&self.import_name_input, cx);
        let java = Self::value(&self.java_input, cx);
        let mirror = Self::value(&self.mirror_input, cx);
        let maven = Self::value(&self.maven_input, cx);

        let mut args = vec![
            String::from("import"),
            String::from("--dir"),
            dir,
            String::from("--file"),
            file,
        ];
        push_option(&mut args, "--name", &name);
        push_option(&mut args, "--java", &java);
        push_option(&mut args, "--mirror", &mirror);
        push_option(&mut args, "--maven", &maven);
        if self.import_optional {
            args.push(String::from("--with-optional"));
        }

        let instance = (!name.is_empty()).then(|| SharedString::from(name.clone()));
        self.start_task(
            self.t("task.title.import"),
            instance,
            args,
            true,
            None,
            cx,
        );
    }

    fn import_modrinth(&mut self, cx: &mut Context<Self>) {
        let dir = Self::value(&self.dir_input, cx);
        if dir.is_empty() {
            return;
        }
        let slug = Self::value(&self.mod_slug_input, cx);
        if slug.is_empty() {
            return;
        }
        let name = Self::value(&self.import_name_input, cx);
        let java = Self::value(&self.java_input, cx);
        let mirror = Self::value(&self.mirror_input, cx);
        let maven = Self::value(&self.maven_input, cx);

        let mut args = vec![
            String::from("import"),
            String::from("--dir"),
            dir,
            String::from("--modrinth"),
            slug,
        ];
        push_option(&mut args, "--name", &name);
        push_option(&mut args, "--java", &java);
        push_option(&mut args, "--mirror", &mirror);
        push_option(&mut args, "--maven", &maven);
        if self.import_optional {
            args.push(String::from("--with-optional"));
        }

        let instance = (!name.is_empty()).then(|| SharedString::from(name.clone()));
        self.start_task(
            self.t("task.title.import"),
            instance,
            args,
            true,
            None,
            cx,
        );
    }

    fn search_type(&self) -> &'static str {
        match self.download_tab {
            1 => "modpack",
            3 => "resourcepack",
            4 => "shader",
            5 => "datapack",
            _ => "mod",
        }
    }

    fn search_mods(&mut self, cx: &mut Context<Self>) {
        let query = Self::value(&self.mod_query_input, cx);
        if query.is_empty() {
            return;
        }
        let limit = Self::value(&self.mod_limit_input, cx);
        if limit.is_empty() {
            return;
        }
        let mut args = vec![
            String::from("search"),
            String::from("--query"),
            query.clone(),
            String::from("--type"),
            String::from(self.search_type()),
            String::from("--limit"),
            limit,
        ];
        if matches!(self.download_tab, 2 | 4) {
            let loader = Self::value(&self.mod_loader_input, cx);
            push_option(&mut args, "--loader", &loader);
        }
        let version = Self::value(&self.mod_version_input, cx);
        push_option(&mut args, "--version", &version);

        let title = self.t_args("task.title.search", &[("query", &query)]);
        self.start_task(
            title,
            None,
            args,
            false,
            None,
            cx,
        );
    }

    fn add_mod(&mut self, cx: &mut Context<Self>) {
        let dir = Self::value(&self.dir_input, cx);
        let name = Self::value(&self.mod_instance_input, cx);
        let slug = Self::value(&self.mod_slug_input, cx);
        if dir.is_empty() || name.is_empty() || slug.is_empty() {
            return;
        }
        let args = vec![
            String::from("add"),
            String::from("--dir"),
            dir,
            String::from("--name"),
            name.clone(),
            String::from("--slug"),
            slug.clone(),
            String::from("--type"),
            String::from(self.search_type()),
        ];
        let title = self.t_args("task.title.add", &[("slug", &slug)]);
        self.start_task(
            title,
            Some(SharedString::from(name)),
            args,
            false,
            None,
            cx,
        );
    }

    fn open_instance_settings(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        let dir = Self::value(&self.dir_input, cx);
        if dir.is_empty() {
            return;
        }
        let config = instance::read_launch_config(&dir, name);
        self.cfg_name = SharedString::from(name.to_string());

        let username = config
            .username
            .unwrap_or_else(|| Self::value(&self.username_input, cx));
        Self::set_input(&self.cfg_username_input, &username, window, cx);
        let memory = config
            .memory
            .unwrap_or_else(|| self.memory_value(&self.memory_input, cx).unwrap_or(4096))
            .to_string();
        Self::set_input(&self.cfg_memory_input, &memory, window, cx);
        let java = config
            .java
            .unwrap_or_else(|| Self::value(&self.java_input, cx));
        Self::set_input(&self.cfg_java_input, &java, window, cx);
        let mirror = config
            .mirror
            .unwrap_or_else(|| Self::value(&self.mirror_input, cx));
        Self::set_input(&self.cfg_mirror_input, &mirror, window, cx);
        let maven = config
            .maven
            .unwrap_or_else(|| Self::value(&self.maven_input, cx));
        Self::set_input(&self.cfg_maven_input, &maven, window, cx);
        self.cfg_isolate = config.isolate.unwrap_or(self.isolate);

        self.instance_page = 2;
        cx.notify();
    }

    fn save_instance_settings(&mut self, cx: &mut Context<Self>) {
        let dir = Self::value(&self.dir_input, cx);
        let name = self.cfg_name.to_string();
        if dir.is_empty() || name.is_empty() {
            return;
        }
        if Self::value(&self.cfg_memory_input, cx).is_empty() {
            return;
        }
        let config = LaunchConfig {
            username: Some(Self::value(&self.cfg_username_input, cx)),
            memory: self.memory_value(&self.cfg_memory_input, cx),
            java: Some(Self::value(&self.cfg_java_input, cx)),
            mirror: Some(Self::value(&self.cfg_mirror_input, cx)),
            maven: Some(Self::value(&self.cfg_maven_input, cx)),
            isolate: Some(self.cfg_isolate),
        };
        let _ = instance::write_launch_config(&dir, &name, &config);
        self.reload(cx);
    }

    fn save_settings(&mut self, cx: &mut Context<Self>) {
        if Self::value(&self.memory_input, cx).is_empty() {
            return;
        }
        let value = self.settings_value(cx);
        let path = config_path();
        let _ = write_json(&path, &value, self.lang);
        self.reload(cx);
    }
}

struct LaunchParams {
    username: String,
    memory: Option<u32>,
    java: String,
    mirror: String,
    maven: String,
    isolate: bool,
}

fn input(
    window: &mut Window,
    value: &str,
    cx: &mut Context<LauncherUi>,
) -> Entity<input::InputState> {
    cx.new(|cx| {
        let value = value.to_string();
        input::InputState::new(window, cx).default_value(value)
    })
}

fn kill_tree(mut child: std::process::Child) {
    #[cfg(windows)]
    {
        let pid = child.id().to_string();
        let _ = Command::new("taskkill")
            .args(["/PID", pid.as_str(), "/T", "/F"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    let _ = child.kill();
    let _ = child.wait();
}

fn execute(
    exe: PathBuf,
    args: Vec<String>,
    tx: channel::Sender<TaskEvent>,
    task: Arc<TaskHandle>,
    lang: Lang,
) -> bool {
    let child = Command::new(exe)
        .args(&args)
        .env("LYYS_GUI", "1")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();
    let Ok(mut child) = child else {
        let _ = tx.send_blocking(TaskEvent::Log(
            i18n::text(lang, "log.start_failed").to_string(),
        ));
        let _ = tx.send_blocking(TaskEvent::Done(false));
        return false;
    };

    if let Some(stdout) = child.stdout.take() {
        let tx = tx.clone();
        std::thread::spawn(move || pump(stdout, tx));
    }
    if let Some(stderr) = child.stderr.take() {
        let tx = tx.clone();
        std::thread::spawn(move || pump(stderr, tx));
    }

    {
        let Ok(mut slot) = task.child.lock() else {
            let _ = tx.send_blocking(TaskEvent::Done(false));
            return false;
        };
        *slot = Some(child);
    }

    loop {
        let state = {
            let Ok(mut slot) = task.child.lock() else {
                let _ = tx.send_blocking(TaskEvent::Done(false));
                return false;
            };
            match slot.as_mut() {
                Some(child) => match child.try_wait() {
                    Ok(Some(status)) => Some(Ok(status)),
                    Ok(None) => None,
                    Err(error) => Some(Err(error)),
                },
                None => Some(Err(std::io::Error::other("cancelled"))),
            }
        };

        match state {
            None => std::thread::sleep(Duration::from_millis(100)),
            Some(Ok(status)) => {
                let success = status.success();
                let _ = tx.send_blocking(TaskEvent::Log(
                    i18n::text_args(lang, "log.exit", &[("status", &status.to_string())])
                        .to_string(),
                ));
                let _ = tx.send_blocking(TaskEvent::Done(success));
                return success;
            }
            Some(Err(error)) => {
                let message = if task.cancelled.load(Ordering::SeqCst) {
                    i18n::text(lang, "log.cancelled").to_string()
                } else {
                    i18n::text_args(lang, "log.wait_failed", &[("error", &error.to_string())])
                        .to_string()
                };
                let _ = tx.send_blocking(TaskEvent::Log(message));
                let _ = tx.send_blocking(TaskEvent::Done(false));
                return false;
            }
        }
    }
}

fn pump<R: Read + Send + 'static>(reader: R, tx: channel::Sender<TaskEvent>) {
    let mut reader = BufReader::new(reader);
    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => break,
            Ok(_) => {
                if line.ends_with('\n') {
                    line.pop();
                    if line.ends_with('\r') {
                        line.pop();
                    }
                }
                if line.is_empty() {
                    continue;
                }
                let event = match line.strip_prefix("#progress ") {
                    Some(rest) => rest
                        .split_once('/')
                        .and_then(|(done, total)| {
                            Some(TaskEvent::Progress {
                                done: done.trim().parse().ok()?,
                                total: total.trim().parse().ok()?,
                            })
                        })
                        .unwrap_or_else(|| TaskEvent::Log(line.clone())),
                    None => TaskEvent::Log(line.clone()),
                };
                if tx.send_blocking(event).is_err() {
                    break;
                }
            }
            Err(_) => break,
        }
    }
}

fn push_option(args: &mut Vec<String>, flag: &str, value: &str) {
    if !value.is_empty() {
        args.push(flag.to_string());
        args.push(value.to_string());
    }
}

fn list_instances(mc_dir: &str) -> Vec<InstanceRow> {
    let versions = Path::new(mc_dir).join("versions");
    let Ok(entries) = std::fs::read_dir(&versions) else {
        return Vec::new();
    };

    let mut rows = Vec::new();
    for entry in entries.flatten() {
        if !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if !versions.join(&name).join(format!("{name}.json")).is_file() {
            continue;
        }

        let (version, loader) = match crate::version::instance_info(mc_dir, &name) {
            Ok(info) => (
                info.mc_version,
                info.loader.unwrap_or_else(|| String::from("vanilla")),
            ),
            Err(_) => (String::from("unknown"), String::from("unknown")),
        };
        let isolated = instance::is_isolated(mc_dir, &name);
        rows.push(InstanceRow {
            name: SharedString::from(name),
            version: SharedString::from(version),
            loader: SharedString::from(loader),
            isolated,
        });
    }

    rows.sort_by(|left, right| left.name.cmp(&right.name));
    rows
}

struct Settings {
    game_dir: String,
    username: String,
    memory: Option<u32>,
    java: String,
    mirror: String,
    maven: String,
    isolate: bool,
    language: Option<String>,
}

impl Settings {
    fn fallback() -> Self {
        Settings {
            game_dir: String::new(),
            username: String::from("Player"),
            memory: Some(4096),
            java: String::new(),
            mirror: String::new(),
            maven: String::new(),
            isolate: false,
            language: None,
        }
    }
}

fn load_settings() -> Settings {
    let Ok(text) = std::fs::read_to_string(config_path()) else {
        return Settings::fallback();
    };
    let Ok(value) = serde_json::from_str::<Value>(&text) else {
        return Settings::fallback();
    };
    let default = Settings::fallback();

    Settings {
        game_dir: value["game_dir"]
            .as_str()
            .unwrap_or(&default.game_dir)
            .to_string(),
        username: value["username"]
            .as_str()
            .unwrap_or(&default.username)
            .to_string(),
        memory: value["memory"]
            .as_u64()
            .and_then(|value| u32::try_from(value).ok()),
        java: value["java"].as_str().unwrap_or("").to_string(),
        mirror: value["mirror"].as_str().unwrap_or("").to_string(),
        maven: value["maven"].as_str().unwrap_or("").to_string(),
        isolate: value["isolate"].as_bool().unwrap_or(false),
        language: value["language"].as_str().map(str::to_string),
    }
}

fn config_path() -> PathBuf {
    crate::loader::base_dir()
        .join("lyys_mc_launcher")
        .join("config.json")
}

fn write_json(path: &Path, value: &Value, lang: Lang) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| {
            i18n::text_args(lang, "error.create_dir", &[("error", &e.to_string())]).to_string()
        })?;
    }
    let text = serde_json::to_string_pretty(value).map_err(|e| e.to_string())?;
    std::fs::write(path, text).map_err(|e| {
        i18n::text_args(lang, "error.write_file", &[("error", &e.to_string())]).to_string()
    })
}

impl Render for LauncherUi {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .size_full()
            .bg(skin::c(skin::WINDOW))
            .text_color(skin::c(skin::TEXT))
            .text_size(px(13.))
            .child(self.render_titlebar(cx))
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .child(self.render_sidebar(cx))
                    .child(self.render_main(cx)),
            )
    }
}

fn dot(color: u32, size: f32) -> Div {
    div()
        .w(px(size))
        .h(px(size))
        .rounded_full()
        .flex_shrink_0()
        .bg(skin::c(color))
}

fn panel() -> Div {
    div()
        .w_full()
        .flex()
        .flex_col()
        .rounded(px(12.))
        .bg(skin::c(skin::CARD))
        .overflow_hidden()
}

fn row() -> Div {
    h_flex()
        .w_full()
        .items_center()
        .justify_between()
        .gap(px(24.))
        .px(px(20.))
        .py(px(14.))
}

fn row_text(title: impl Into<SharedString>, desc: impl Into<SharedString>) -> Div {
    let title = title.into();
    let desc = desc.into();
    v_flex()
        .min_w_0()
        .gap(px(3.))
        .child(
            div()
                .text_size(px(13.5))
                .font_medium()
                .text_color(skin::c(skin::TEXT))
                .child(title),
        )
        .child(
            div()
                .text_size(px(12.5))
                .text_color(skin::c(skin::MUTED))
                .child(desc),
        )
}

fn row_title(title: impl Into<SharedString>) -> Div {
    div()
        .min_w_0()
        .text_size(px(13.5))
        .font_medium()
        .text_color(skin::c(skin::TEXT))
        .child(title.into())
}

fn sep() -> Div {
    div().h(px(1.)).mx(px(20.)).bg(skin::c(skin::DIVIDER))
}

fn page_title(title: impl Into<SharedString>) -> Div {
    div()
        .text_size(px(20.))
        .font_semibold()
        .text_color(skin::c(skin::TEXT))
        .child(title.into())
}

fn primary_button(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Stateful<Div> {
    let label = label.into();
    div()
        .id(id)
        .h(px(30.))
        .px(px(12.))
        .rounded(px(7.))
        .flex()
        .items_center()
        .bg(skin::c(skin::ACCENT))
        .text_color(skin::c(skin::ACCENT_FG))
        .text_size(px(12.5))
        .font_medium()
        .cursor(CursorStyle::PointingHand)
        .hover(|style| style.bg(skin::c(skin::ACCENT_HOVER)))
        .child(label)
}

fn text_button(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Stateful<Div> {
    let label = label.into();
    div()
        .id(id)
        .h(px(30.))
        .px(px(10.))
        .rounded(px(7.))
        .flex()
        .items_center()
        .text_size(px(12.5))
        .text_color(skin::c(skin::MUTED))
        .cursor(CursorStyle::PointingHand)
        .hover(|style| {
            style
                .text_color(skin::c(skin::TEXT))
                .bg(skin::c(skin::HOVER))
        })
        .child(label)
}

fn danger_button(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Stateful<Div> {
    let label = label.into();
    div()
        .id(id)
        .h(px(30.))
        .px(px(12.))
        .rounded(px(7.))
        .flex()
        .items_center()
        .bg(skin::c(skin::DANGER))
        .text_color(skin::c(skin::DANGER_FG))
        .text_size(px(12.5))
        .font_medium()
        .cursor(CursorStyle::PointingHand)
        .hover(|style| style.bg(skin::c(skin::DANGER_HOVER)))
        .child(label)
}

impl LauncherUi {
    fn render_titlebar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let running = self
            .tasks
            .iter()
            .filter(|task| task.state == TaskState::Running)
            .count();
        let mut button = text_button("nav-tasks", self.t("nav.tasks")).on_click(cx.listener(
            |this, _, _, cx| {
                this.tab = 3;
                this.instance_page = 0;
                cx.notify();
            },
        ));
        if running > 0 {
            button = button.child(
                div()
                    .ml(px(6.))
                    .px(px(6.))
                    .h(px(16.))
                    .rounded_full()
                    .flex()
                    .items_center()
                    .bg(skin::c(skin::BUSY))
                    .text_size(px(10.5))
                    .text_color(skin::c(skin::WINDOW))
                    .child(running.to_string()),
            );
        }
        TitleBar::new()
            .bg(skin::c(skin::WINDOW))
            .border_b_1()
            .border_color(skin::c(skin::TITLEBAR_LINE))
            .child(div().flex_1())
            .child(
                div()
                    .pr(px(8.))
                    .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(button),
            )
    }

    fn render_sidebar(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .w(px(236.))
            .h_full()
            .flex_shrink_0()
            .bg(skin::c(skin::PANEL))
            .px(px(12.))
            .pt(px(6.))
            .pb(px(14.))
            .child(self.side_nav(
                "nav-instances",
                IconName::LayoutDashboard,
                self.t("nav.instances"),
                self.tab == 0,
                0,
                cx,
            ))
            .child(self.side_nav(
                "nav-download",
                IconName::ArrowDown,
                self.t("nav.download"),
                self.tab == 1,
                1,
                cx,
            ))
            .child(
                h_flex().mt(px(18.)).px(px(10.)).pb(px(6.)).child(
                    div()
                        .text_size(px(13.))
                        .text_color(skin::c(skin::MUTED))
                        .child(self.t("nav.section.system")),
                ),
            )
            .child(self.side_nav(
                "nav-settings",
                IconName::Settings,
                self.t("nav.settings"),
                self.tab == 2,
                2,
                cx,
            ))
            .child(div().flex_1())
            .child(
                v_flex().px(px(10.)).gap(px(2.)).child(
                    div()
                        .text_size(px(11.5))
                        .text_color(skin::c(skin::FAINT))
                        .child(env!("CARGO_PKG_VERSION")),
                ),
            )
    }

    #[allow(clippy::too_many_arguments)]
    fn side_nav(
        &self,
        id: &'static str,
        icon: IconName,
        label: impl Into<SharedString>,
        active: bool,
        index: usize,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let label = label.into();
        let foreground = if active {
            skin::c(skin::TEXT)
        } else {
            skin::c(skin::SIDEBAR_TEXT)
        };

        h_flex()
            .id(id)
            .w_full()
            .items_center()
            .gap(px(10.))
            .h(px(36.))
            .px(px(10.))
            .rounded(px(7.))
            .when(active, |this| this.bg(skin::c(skin::ACTIVE)))
            .hover(|style| style.bg(skin::c(skin::HOVER)))
            .cursor(CursorStyle::PointingHand)
            .on_click(cx.listener(move |this, _event, _window, cx| {
                this.tab = index;
                this.instance_page = 0;
                this.download_tab = 0;
                cx.notify();
            }))
            .child(Icon::new(icon).with_size(px(16.)).text_color(foreground))
            .child(
                div()
                    .text_size(px(13.5))
                    .text_color(foreground)
                    .child(label),
            )
    }

    fn render_main(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .bg(skin::c(skin::CONTENT))
            .child(
                h_flex().w_full().flex_1().min_h_0().justify_center().child(
                    v_flex()
                        .w_full()
                        .max_w(px(820.))
                        .h_full()
                        .min_h_0()
                        .px(px(40.))
                        .pt(px(28.))
                        .child(self.render_page(cx)),
                ),
            )
    }

    fn render_page(&mut self, cx: &mut Context<Self>) -> AnyElement {
        match self.instance_page {
            2 => self.render_instance_settings_page(cx).into_any_element(),
            _ => match self.tab {
                0 => self.render_instances(cx).into_any_element(),
                1 => self.render_download(cx).into_any_element(),
                3 => self.render_tasks(cx).into_any_element(),
                _ => self.render_general_settings(cx).into_any_element(),
            },
        }
    }

    fn render_instances(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let mut list = panel();

        if self.instances.is_empty() {
            list = list.child(
                h_flex().w_full().justify_center().py(px(40.)).child(
                    div()
                        .text_size(px(13.))
                        .text_color(skin::c(skin::FAINT))
                        .child(self.t("instances.empty")),
                ),
            );
        }

        for (index, row_data) in self.instances.clone().into_iter().enumerate() {
            let launch_name = row_data.name.to_string();
            let launch_click = launch_name.clone();
            let folder_name = row_data.name.clone();
            let settings_name = row_data.name.clone();
            let mut meta = format!("{} · {}", row_data.version, row_data.loader);
            if row_data.isolated {
                meta.push_str(&format!(" · {}", self.t("instances.meta.isolated")));
            }
            if index > 0 {
                list = list.child(sep());
            }
            let running = self.running_task_for(&launch_name);
            list =
                list.child(
                    row().child(row_text(row_data.name.clone(), meta)).child(
                        h_flex()
                            .flex_shrink_0()
                            .items_center()
                            .gap(px(4.))
                            .child(if let Some(task_id) = running {
                                danger_button(
                                    SharedString::from(format!("cancel-{launch_name}")),
                                    self.t("instances.action.cancel"),
                                )
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.cancel_task(task_id, cx)
                                }))
                                .into_any_element()
                            } else {
                                primary_button(
                                    SharedString::from(format!("launch-{launch_name}")),
                                    self.t("instances.action.launch"),
                                )
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.launch_instance(&launch_click, cx)
                                }))
                                .into_any_element()
                            })
                            .child(
                                text_button(
                                    SharedString::from(format!("folder-{launch_name}")),
                                    self.t("instances.action.folder"),
                                )
                                .on_click(cx.listener(
                                    move |this, _, _, cx| this.open_folder(&folder_name, cx),
                                )),
                            )
                            .child(
                                text_button(
                                    SharedString::from(format!("settings-{launch_name}")),
                                    self.t("instances.action.settings"),
                                )
                                .on_click(cx.listener(
                                    move |this, _, window, cx| {
                                        this.open_instance_settings(&settings_name, window, cx)
                                    },
                                )),
                            ),
                    ),
                );
        }

        v_flex()
            .size_full()
            .gap(px(20.))
            .child(
                h_flex()
                    .w_full()
                    .items_start()
                    .justify_between()
                    .gap(px(16.))
                    .child(page_title(self.t("page.instances.title")))
                    .child(
                        h_flex()
                            .flex_shrink_0()
                            .items_center()
                            .gap(px(6.))
                            .pt(px(4.))
                            .child(text_button("refresh", self.t("common.refresh")).on_click(
                                cx.listener(|this, _, _, cx| this.reload(cx)),
                            )),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .id("instance-list")
                    .overflow_y_scroll()
                    .pb(px(6.))
                    .child(list),
            )
    }

    fn render_download(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let mut body = v_flex().gap(px(16.));
        let content_tab = self.download_tab;
        let show_loader = content_tab == 2 || content_tab == 4;
        if self.download_tab == 0 {
            body = body.child(self.render_install_form(cx));
        } else if self.download_tab == 1 {
            body = body
                .child(
                    panel()
                        .child(
                            row()
                                .child(row_title(self.t("download.import.file.title")))
                                .child(self.input_control(&self.import_file_input)),
                        )
                        .child(sep())
                        .child(
                            row()
                                .child(row_title(self.t("download.import.name.title")))
                                .child(self.input_control(&self.import_name_input)),
                        )
                        .child(sep())
                        .child(
                            row()
                                .child(row_title(self.t("download.import.optional.title")))
                                .child(
                                    Switch::new("import-optional")
                                        .checked(self.import_optional)
                                        .on_click(cx.listener(|this, checked: &bool, _, cx| {
                                            this.import_optional = *checked;
                                            cx.notify();
                                        })),
                                ),
                        ),
                )
                .child(h_flex().w_full().justify_end().child(
                    primary_button("import-run", self.t("download.import.run")).on_click(
                        cx.listener(|this, _, _, cx| this.import_pack(cx)),
                    ),
                ))
                .child(
                    panel()
                        .child(
                            row()
                                .child(row_title(self.t("download.pack.query.title")))
                                .child(self.input_control(&self.mod_query_input)),
                        )
                        .child(sep())
                        .child(
                            row()
                                .child(row_title(self.t("download.version.title")))
                                .child(self.input_control(&self.mod_version_input)),
                        )
                        .child(sep())
                        .child(
                            row()
                                .child(row_title(self.t("download.limit.title")))
                                .child(self.input_control(&self.mod_limit_input)),
                        ),
                )
                .child(h_flex().w_full().justify_end().child(
                    primary_button("pack-search-run", self.t("download.pack.search")).on_click(
                        cx.listener(|this, _, _, cx| this.search_mods(cx)),
                    ),
                ))
                .child(
                    panel().child(
                        row()
                            .child(row_title(self.t("download.pack.slug.title")))
                            .child(self.input_control(&self.mod_slug_input)),
                    ),
                )
                .child(h_flex().w_full().justify_end().child(
                    primary_button("pack-import-run", self.t("download.pack.import")).on_click(
                        cx.listener(|this, _, _, cx| this.import_modrinth(cx)),
                    ),
                ));
        } else {
            let mut search = panel().child(
                row()
                    .child(row_title(self.t("download.pack.query.title")))
                    .child(self.input_control(&self.mod_query_input)),
            );
            if show_loader {
                let label = if content_tab == 4 {
                    row_title(self.t("download.content.shader_loader.title"))
                } else {
                    row_title(self.t("download.content.loader.title"))
                };
                let control = if content_tab == 2 {
                    let current = Self::value(&self.mod_loader_input, cx);
                    h_flex()
                        .flex_shrink_0()
                        .gap(px(2.))
                        .p(px(3.))
                        .rounded(px(9.))
                        .bg(skin::c(skin::PANEL))
                        .child(Self::loader_option(
                            "mod-loader-fabric",
                            "fabric",
                            current == "fabric",
                            self.mod_loader_input.clone(),
                            cx,
                        ))
                        .child(Self::loader_option(
                            "mod-loader-forge",
                            "forge",
                            current == "forge",
                            self.mod_loader_input.clone(),
                            cx,
                        ))
                        .child(Self::loader_option(
                            "mod-loader-neoforge",
                            "neoforge",
                            current == "neoforge",
                            self.mod_loader_input.clone(),
                            cx,
                        ))
                } else if content_tab == 4 {
                    let current = Self::value(&self.mod_loader_input, cx);
                    h_flex()
                        .flex_shrink_0()
                        .gap(px(2.))
                        .p(px(3.))
                        .rounded(px(9.))
                        .bg(skin::c(skin::PANEL))
                        .child(Self::loader_option(
                            "shader-loader-iris",
                            "iris",
                            current == "iris",
                            self.mod_loader_input.clone(),
                            cx,
                        ))
                        .child(Self::loader_option(
                            "shader-loader-optifine",
                            "optifine",
                            current == "optifine",
                            self.mod_loader_input.clone(),
                            cx,
                        ))
                } else {
                    div().child(self.input_control(&self.mod_loader_input))
                };
                search = search.child(sep()).child(row().child(label).child(control));
            }
            body = body
                .child(
                    search
                        .child(sep())
                        .child(
                            row()
                                .child(row_title(self.t("download.version.title")))
                                .child(self.input_control(&self.mod_version_input)),
                        )
                        .child(sep())
                        .child(
                            row()
                                .child(row_title(self.t("download.limit.title")))
                                .child(self.input_control(&self.mod_limit_input)),
                        ),
                )
                .child(h_flex().w_full().justify_end().child(
                    primary_button("search-run", self.t("download.content.search")).on_click(
                        cx.listener(|this, _, _, cx| this.search_mods(cx)),
                    ),
                ))
                .child(
                    panel()
                        .child(
                            row()
                                .child(row_title(self.t("download.pack.slug.title")))
                                .child(self.input_control(&self.mod_slug_input)),
                        )
                        .child(sep())
                        .child(
                            row()
                                .child(row_title(self.t("download.content.instance.title")))
                                .child(self.input_control(&self.mod_instance_input)),
                        ),
                )
                .child(h_flex().w_full().justify_end().child(
                    primary_button("add-run", self.t("download.content.add")).on_click(
                        cx.listener(|this, _, _, cx| this.add_mod(cx)),
                    ),
                ));
        }

        v_flex()
            .size_full()
            .gap(px(20.))
            .child(
                h_flex()
                    .w_full()
                    .items_start()
                    .justify_between()
                    .gap(px(16.))
                    .child(page_title(self.t("page.download.title")))
                    .child(
                        h_flex()
                            .flex_shrink_0()
                            .gap(px(2.))
                            .p(px(3.))
                            .rounded(px(9.))
                            .bg(skin::c(skin::PANEL))
                            .child(self.segment(
                                "tab-instance",
                                self.t("download.tab.instance"),
                                self.download_tab == 0,
                                0,
                                cx,
                            ))
                            .child(self.segment(
                                "tab-pack",
                                self.t("download.tab.pack"),
                                self.download_tab == 1,
                                1,
                                cx,
                            ))
                            .child(self.segment(
                                "tab-mods",
                                self.t("download.tab.mods"),
                                self.download_tab == 2,
                                2,
                                cx,
                            ))
                            .child(self.segment(
                                "tab-resourcepack",
                                self.t("download.tab.resourcepack"),
                                self.download_tab == 3,
                                3,
                                cx,
                            ))
                            .child(self.segment(
                                "tab-shader",
                                self.t("download.tab.shader"),
                                self.download_tab == 4,
                                4,
                                cx,
                            ))
                            .child(self.segment(
                                "tab-datapack",
                                self.t("download.tab.datapack"),
                                self.download_tab == 5,
                                5,
                                cx,
                            )),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .id("download-scroll")
                    .overflow_y_scroll()
                    .pb(px(6.))
                    .child(body),
            )
    }

    fn segment(
        &self,
        id: &'static str,
        label: impl Into<SharedString>,
        active: bool,
        index: usize,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let label = label.into();
        div()
            .id(id)
            .h(px(26.))
            .px(px(12.))
            .rounded(px(7.))
            .flex()
            .items_center()
            .text_size(px(12.5))
            .when(active, |this| {
                this.bg(skin::c(skin::ACTIVE))
                    .text_color(skin::c(skin::TEXT))
            })
            .when(!active, |this| this.text_color(skin::c(skin::MUTED)))
            .cursor(CursorStyle::PointingHand)
            .on_click(cx.listener(move |this, _event, window, cx| {
                this.download_tab = index;
                if index == 2 {
                    let current = Self::value(&this.mod_loader_input, cx);
                    if !matches!(current.as_str(), "fabric" | "forge" | "neoforge") {
                        Self::set_input(&this.mod_loader_input, "fabric", window, cx);
                    }
                }
                if index == 4 {
                    let current = Self::value(&this.mod_loader_input, cx);
                    if current != "iris" && current != "optifine" {
                        Self::set_input(&this.mod_loader_input, "iris", window, cx);
                    }
                }
                cx.notify();
            }))
            .child(label)
    }

    fn loader_option(
        id: &'static str,
        value: &'static str,
        active: bool,
        target: Entity<input::InputState>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .id(id)
            .h(px(26.))
            .px(px(12.))
            .rounded(px(7.))
            .flex()
            .items_center()
            .text_size(px(12.5))
            .when(active, |this| {
                this.bg(skin::c(skin::ACTIVE))
                    .text_color(skin::c(skin::TEXT))
            })
            .when(!active, |this| this.text_color(skin::c(skin::MUTED)))
            .cursor(CursorStyle::PointingHand)
            .on_click(cx.listener(move |_this, _event, window, cx| {
                Self::set_input(&target, if active { "" } else { value }, window, cx);
                cx.notify();
            }))
            .child(value)
    }

    fn lang_option(
        &self,
        id: &'static str,
        lang: Lang,
        active: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .id(id)
            .h(px(26.))
            .px(px(12.))
            .rounded(px(7.))
            .flex()
            .items_center()
            .text_size(px(12.5))
            .when(active, |this| {
                this.bg(skin::c(skin::ACTIVE))
                    .text_color(skin::c(skin::TEXT))
            })
            .when(!active, |this| this.text_color(skin::c(skin::MUTED)))
            .cursor(CursorStyle::PointingHand)
            .on_click(cx.listener(move |this, _event, _window, cx| {
                this.select_language(lang, cx);
            }))
            .child(lang.native_name())
    }

    fn render_general_settings(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .size_full()
            .gap(px(20.))
            .child(page_title(self.t("page.settings.title")))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .id("settings-scroll")
                    .overflow_y_scroll()
                    .pb(px(6.))
                    .child(
                        v_flex()
                            .gap(px(16.))
                            .child(
                                panel()
                                    .child(
                                        row()
                                            .child(row_title(self.t("settings.game_dir.title")))
                                            .child(self.input_control(&self.dir_input)),
                                    )
                                    .child(sep())
                                    .child(
                                        row()
                                            .child(row_title(self.t("settings.username.title")))
                                            .child(self.input_control(&self.username_input)),
                                    )
                                    .child(sep())
                                    .child(
                                        row()
                                            .child(row_title(self.t("settings.memory.title")))
                                            .child(self.input_control(&self.memory_input)),
                                    ),
                            )
                            .child(
                                panel()
                                    .child(
                                        row()
                                            .child(row_title(self.t("settings.java.title")))
                                            .child(self.input_control(&self.java_input)),
                                    )
                                    .child(sep())
                                    .child(
                                        row()
                                            .child(row_title(self.t("settings.mirror.title")))
                                            .child(self.input_control(&self.mirror_input)),
                                    )
                                    .child(sep())
                                    .child(
                                        row()
                                            .child(row_title(self.t("settings.maven.title")))
                                            .child(self.input_control(&self.maven_input)),
                                    ),
                            )
                            .child(
                                panel().child(
                                    row()
                                        .child(row_title(self.t("settings.language.title")))
                                        .child(
                                            h_flex()
                                                .flex_shrink_0()
                                                .gap(px(2.))
                                                .p(px(3.))
                                                .rounded(px(9.))
                                                .bg(skin::c(skin::PANEL))
                                                .child(self.lang_option(
                                                    "lang-zh",
                                                    Lang::ZhCn,
                                                    self.lang == Lang::ZhCn,
                                                    cx,
                                                ))
                                                .child(self.lang_option(
                                                    "lang-en",
                                                    Lang::EnUs,
                                                    self.lang == Lang::EnUs,
                                                    cx,
                                                ))
                                                .child(self.lang_option(
                                                    "lang-ja",
                                                    Lang::JaJp,
                                                    self.lang == Lang::JaJp,
                                                    cx,
                                                )),
                                        ),
                                ),
                            )
                            .child(
                                panel().child(
                                    row()
                                        .child(row_title(self.t("settings.isolate.title")))
                                        .child(
                                            Switch::new("settings-isolate")
                                                .checked(self.isolate)
                                                .on_click(cx.listener(
                                                    |this, checked: &bool, _, cx| {
                                                        this.isolate = *checked;
                                                        cx.notify();
                                                    },
                                                )),
                                        ),
                                ),
                            )
                            .child(
                                h_flex().w_full().justify_end().child(
                                    primary_button("save-settings", self.t("settings.save"))
                                        .on_click(
                                            cx.listener(|this, _, _, cx| this.save_settings(cx)),
                                        ),
                                ),
                            ),
                    ),
            )
    }

    fn render_install_form(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let loader = Self::value(&self.install_loader_input, cx);
        v_flex()
            .gap(px(16.))
            .child(
                panel()
                    .child(
                        row()
                            .child(row_title(self.t("install.version.title")))
                            .child(self.input_control(&self.install_version_input)),
                    )
                    .child(sep())
                    .child(
                        row()
                            .child(row_title(self.t("install.name.title")))
                            .child(self.input_control(&self.install_name_input)),
                    )
                    .child(sep())
                    .child(
                        row()
                            .child(row_title(self.t("install.loader.title")))
                            .child(
                                h_flex()
                                    .flex_shrink_0()
                                    .gap(px(2.))
                                    .p(px(3.))
                                    .rounded(px(9.))
                                    .bg(skin::c(skin::PANEL))
                                    .child(Self::loader_option(
                                        "install-loader-fabric",
                                        "fabric",
                                        loader == "fabric",
                                        self.install_loader_input.clone(),
                                        cx,
                                    ))
                                    .child(Self::loader_option(
                                        "install-loader-forge",
                                        "forge",
                                        loader == "forge",
                                        self.install_loader_input.clone(),
                                        cx,
                                    ))
                                    .child(Self::loader_option(
                                        "install-loader-neoforge",
                                        "neoforge",
                                        loader == "neoforge",
                                        self.install_loader_input.clone(),
                                        cx,
                                    )),
                            ),
                    )
                    .child(sep())
                    .child(
                        row()
                            .child(row_title(self.t("install.loader_version.title")))
                            .child(self.input_control(&self.install_loader_version_input)),
                    ),
            )
            .child(
                panel()
                    .child(
                        row()
                            .child(row_title(self.t("settings.username.title")))
                            .child(self.input_control(&self.install_username_input)),
                    )
                    .child(sep())
                    .child(
                        row()
                            .child(row_title(self.t("settings.memory.title")))
                            .child(self.input_control(&self.install_memory_input)),
                    )
                    .child(sep())
                    .child(
                        row()
                            .child(row_title(self.t("settings.java.title")))
                            .child(self.input_control(&self.install_java_input)),
                    )
                    .child(sep())
                    .child(
                        row()
                            .child(row_title(self.t("settings.mirror.title")))
                            .child(self.input_control(&self.install_mirror_input)),
                    )
                    .child(sep())
                    .child(
                        row()
                            .child(row_title(self.t("settings.maven.title")))
                            .child(self.input_control(&self.install_maven_input)),
                    ),
            )
            .child(
                panel().child(
                    row()
                        .child(row_title(self.t("install.isolate.title")))
                        .child(
                            Switch::new("install-isolate")
                                .checked(self.install_isolate)
                                .on_click(cx.listener(|this, checked: &bool, _, cx| {
                                    this.install_isolate = *checked;
                                    cx.notify();
                                })),
                        ),
                ),
            )
            .child(h_flex().w_full().justify_end().child(
                primary_button("install-run", self.t("install.run")).on_click(
                    cx.listener(|this, _, _, cx| this.install_version(cx)),
                ),
            ))
    }

    fn render_instance_settings_page(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .size_full()
            .gap(px(20.))
            .child(
                h_flex()
                    .w_full()
                    .items_start()
                    .gap(px(10.))
                    .child(
                        h_flex()
                            .flex_shrink_0()
                            .pt(px(4.))
                            .child(text_button("cfg-back", "←").on_click(
                                cx.listener(|this, _, _, cx| this.back_to_instances(cx)),
                            )),
                    )
                    .child(page_title(self.t("instance_settings.title"))),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .id("cfg-scroll")
                    .overflow_y_scroll()
                    .pb(px(6.))
                    .child(
                        v_flex()
                            .gap(px(16.))
                            .child(
                                panel()
                                    .child(
                                        row()
                                            .child(row_text(
                                                self.t("instance_settings.instance"),
                                                self.cfg_name.clone(),
                                            ))
                                            .child(
                                                div()
                                                    .text_size(px(12.5))
                                                    .text_color(skin::c(skin::MUTED))
                                                    .child(self.t("instance_settings.override")),
                                            ),
                                    )
                                    .child(sep())
                                    .child(
                                        row()
                                            .child(row_title(self.t("settings.username.title")))
                                            .child(self.input_control(&self.cfg_username_input)),
                                    )
                                    .child(sep())
                                    .child(
                                        row()
                                            .child(row_title(self.t("settings.memory.title")))
                                            .child(self.input_control(&self.cfg_memory_input)),
                                    )
                                    .child(sep())
                                    .child(
                                        row()
                                            .child(row_title(self.t("settings.java.title")))
                                            .child(self.input_control(&self.cfg_java_input)),
                                    )
                                    .child(sep())
                                    .child(
                                        row()
                                            .child(row_title(self.t("settings.mirror.title")))
                                            .child(self.input_control(&self.cfg_mirror_input)),
                                    )
                                    .child(sep())
                                    .child(
                                        row()
                                            .child(row_title(self.t("settings.maven.title")))
                                            .child(self.input_control(&self.cfg_maven_input)),
                                    ),
                            )
                            .child(
                                panel().child(
                                    row()
                                        .child(row_title(self.t("instance_settings.isolate.title")))
                                        .child(
                                            Switch::new("cfg-isolate")
                                                .checked(self.cfg_isolate)
                                                .on_click(cx.listener(
                                                    |this, checked: &bool, _, cx| {
                                                        this.cfg_isolate = *checked;
                                                        cx.notify();
                                                    },
                                                )),
                                        ),
                                ),
                            )
                            .child(h_flex().w_full().justify_end().child(
                                primary_button("cfg-save", self.t("instance_settings.save")).on_click(
                                    cx.listener(|this, _, _, cx| this.save_instance_settings(cx)),
                                ),
                            )),
                    ),
            )
    }

    fn input_control(&self, state: &Entity<input::InputState>) -> impl IntoElement {
        input::Input::new(state)
            .w(px(280.))
            .flex_shrink_0()
            .small()
            .bg(skin::c(skin::FIELD))
            .border_1()
            .border_color(skin::c(skin::FIELD_BORDER))
            .rounded(px(8.))
            .text_size(px(12.5))
    }

    fn render_tasks(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let mono = cx.theme().mono_font_family.clone();
        let active = self.active_task;
        let mut list = v_flex()
            .id("task-list")
            .w(px(280.))
            .h_full()
            .flex_shrink_0()
            .gap(px(2.))
            .overflow_y_scroll()
            .pb(px(6.));

        if self.tasks.is_empty() {
            list = list.child(
                div()
                    .text_size(px(12.5))
                    .text_color(skin::c(skin::FAINT))
                    .child(self.t("tasks.empty")),
            );
        }

        for task in self.tasks.iter().rev() {
            let id = task.id;
            let color = match task.state {
                TaskState::Running => skin::BUSY,
                TaskState::Success => skin::OK,
                TaskState::Failed => skin::DANGER_HOVER,
                TaskState::Cancelled => skin::FAINT,
            };
            let state_label = self.t(task_state_key(task.state));
            let selected = active == Some(id);
            let percent = task
                .progress
                .map(|value| format!("{:.0}%", value * 100.0));
            let mut entry = h_flex()
                .id(SharedString::from(format!("task-{id}")))
                .w_full()
                .items_center()
                .gap(px(8.))
                .px(px(10.))
                .py(px(8.))
                .rounded(px(8.))
                .cursor(CursorStyle::PointingHand)
                .when(selected, |this| this.bg(skin::c(skin::ACTIVE)))
                .hover(|style| style.bg(skin::c(skin::HOVER)))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.active_task = Some(id);
                    this.task_log_follow = true;
                    cx.notify();
                }))
                .child(dot(color, 6.))
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .gap(px(3.))
                        .child(
                            div()
                                .truncate()
                                .text_size(px(12.5))
                                .text_color(skin::c(skin::TEXT))
                                .child(task.title.clone()),
                        )
                        .child(
                            div()
                                .text_size(px(11.))
                                .text_color(skin::c(skin::FAINT))
                                .child(state_label),
                        ),
                );
            if let Some(percent) = percent {
                entry = entry.child(
                    div()
                        .text_size(px(11.))
                        .text_color(skin::c(skin::MUTED))
                        .child(percent),
                );
            }
            list = list.child(entry);
        }

        let selected = active
            .and_then(|id| self.tasks.iter().find(|task| task.id == id))
            .map(|task| {
                let skip = task.log_lines.len().saturating_sub(400);
                (
                    task.id,
                    task.title.clone(),
                    task.state,
                    task.progress,
                    task.log_lines.iter().skip(skip).cloned().collect::<Vec<_>>(),
                )
            });

        let mut detail = v_flex().flex_1().min_w_0().h_full().gap(px(14.));
        if let Some((id, title, state, progress, lines)) = selected {
            let state_label = self.t(task_state_key(state));
            let mut header = h_flex()
                .w_full()
                .items_center()
                .justify_between()
                .gap(px(12.))
                .child(
                    v_flex()
                        .min_w_0()
                        .gap(px(4.))
                        .child(
                            div()
                                .truncate()
                                .text_size(px(16.))
                                .text_color(skin::c(skin::TEXT))
                                .child(title),
                        )
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(skin::c(skin::FAINT))
                                .child(self.t_args(
                                    "tasks.detail.header",
                                    &[("id", &id.to_string()), ("state", state_label.as_ref())],
                                )),
                        ),
                );
            if state == TaskState::Running {
                header = header.child(
                    danger_button("task-cancel", self.t("tasks.cancel"))
                        .on_click(cx.listener(move |this, _, _, cx| this.cancel_task(id, cx))),
                );
            }
            detail = detail.child(header);

            if let Some(value) = progress {
                detail = detail.child(
                    h_flex()
                        .items_center()
                        .gap(px(8.))
                        .child(
                            div()
                                .w(px(180.))
                                .h(px(6.))
                                .rounded_full()
                                .bg(skin::c(skin::DIVIDER))
                                .child(
                                    div()
                                        .h_full()
                                        .w(gpui::relative(value))
                                        .rounded_full()
                                        .bg(skin::c(skin::BUSY)),
                                ),
                        )
                        .child(
                            div()
                                .text_size(px(11.5))
                                .text_color(skin::c(skin::MUTED))
                                .child(format!("{:.0}%", value * 100.0)),
                        ),
                );
            }

            if self.task_log_follow {
                self.task_log_follow = false;
                self.task_log_scroll.scroll_to_bottom();
            }

            detail = detail.child(
                div()
                    .id("task-log")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.task_log_scroll)
                    .rounded(px(10.))
                    .bg(skin::c(skin::CARD))
                    .px(px(16.))
                    .py(px(12.))
                    .font_family(mono)
                    .text_size(px(11.5))
                    .text_color(skin::c(skin::MUTED))
                    .child(v_flex().gap(px(3.)).children(if lines.is_empty() {
                        vec![self.t("tasks.log.empty")]
                    } else {
                        lines
                    })),
            );
        } else {
            detail = detail.child(
                div()
                    .text_size(px(12.5))
                    .text_color(skin::c(skin::FAINT))
                    .child(self.t("tasks.detail.empty")),
            );
        }

        v_flex()
            .size_full()
            .gap(px(20.))
            .child(page_title(self.t("page.tasks.title")))
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .gap(px(20.))
                    .child(list)
                    .child(detail),
            )
    }
}
