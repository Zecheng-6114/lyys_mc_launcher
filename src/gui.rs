use std::error::Error;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

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

use crate::instance::{self, LaunchConfig};

mod skin {
    use gpui::{Rgba, rgb};

    /// 窗口外框与标题栏
    pub const WINDOW: u32 = 0x0f0f0f;
    /// 标题栏下方细分隔
    pub const TITLEBAR_LINE: u32 = 0x1f1f1f;
    /// 左侧栏面板
    pub const PANEL: u32 = 0x242424;
    /// 左侧栏正文
    pub const SIDEBAR_TEXT: u32 = 0xd0d0d0;
    /// 主内容区
    pub const CONTENT: u32 = 0x161616;
    /// 卡片
    pub const CARD: u32 = 0x242424;
    /// 卡片内的分隔线
    pub const DIVIDER: u32 = 0x2f2f2f;
    /// 标题栏应用图标底
    pub const APP_ICON: u32 = 0xfafafa;
    /// 悬浮态
    pub const HOVER: u32 = 0x2e2e2e;
    /// 选中态
    pub const ACTIVE: u32 = 0x3a3a3a;
    /// 输入框填充
    pub const FIELD: u32 = 0x2f2f2f;
    /// 输入框描边
    pub const FIELD_BORDER: u32 = 0x3f3f3f;
    /// 正文
    pub const TEXT: u32 = 0xfafafa;
    /// 次要文本
    pub const MUTED: u32 = 0xa3a3a3;
    /// 极弱提示文本
    pub const FAINT: u32 = 0x757575;
    /// 主按钮/强调
    pub const ACCENT: u32 = 0xfafafa;
    pub const ACCENT_HOVER: u32 = 0xffffff;
    pub const ACCENT_ACTIVE: u32 = 0xe4e4e4;
    pub const ACCENT_FG: u32 = 0x121212;
    pub const OK: u32 = 0x5fbf8f;
    pub const BUSY: u32 = 0x7aa2f7;

    pub fn c(hex: u32) -> Rgba {
        rgb(hex)
    }
}

/// 把组件库主题对齐到参考设计的层级：窗口 → 面板 → 卡片 → 控件。
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
    content_tab: usize,
    instances: Vec<InstanceRow>,
    busy: bool,
    status: SharedString,
    log_text: SharedString,
    log_summary: SharedString,
    log_open: bool,
    log_follow: bool,
    log_scroll: ScrollHandle,

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
    mod_type_input: Entity<input::InputState>,
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

        let mut this = LauncherUi {
            tab: 0,
            instance_page: 0,
            content_tab: 0,
            instances: Vec::new(),
            busy: false,
            status: SharedString::from("就绪"),
            log_text: SharedString::default(),
            log_summary: SharedString::default(),
            log_open: false,
            log_follow: false,
            log_scroll: ScrollHandle::new(),

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
            mod_type_input: input(window, "mod", cx),
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

    fn reload(&mut self, cx: &mut Context<Self>) {
        let dir = Self::value(&self.dir_input, cx);
        self.instances = list_instances(&dir);
        cx.notify();
    }

    fn push_log(&mut self, line: String, cx: &mut Context<Self>) {
        self.log_summary = SharedString::from(line.clone());
        self.log_follow = true;
        let mut text = self.log_text.to_string();
        text.push_str(&line);
        text.push('\n');
        if text.chars().count() > 200_000 {
            let skip = text.chars().count() - 200_000;
            text = text.chars().skip(skip).collect();
        }
        self.log_text = SharedString::from(text);
        cx.notify();
    }

    fn run_task(
        &mut self,
        args: Vec<String>,
        refresh: bool,
        save: Option<(String, String, LaunchConfig)>,
        cx: &mut Context<Self>,
    ) {
        if self.busy {
            self.status = SharedString::from("已有任务正在运行");
            cx.notify();
            return;
        }
        let exe = match std::env::current_exe() {
            Ok(path) => path,
            Err(error) => {
                self.status = SharedString::from(format!("无法定位可执行文件: {error}"));
                cx.notify();
                return;
            }
        };
        self.busy = true;
        self.log_open = true;
        self.log_follow = true;
        self.status = SharedString::from("运行中");
        cx.notify();

        let (line_tx, line_rx) = channel::unbounded::<String>();
        let (done_tx, done_rx) = channel::bounded::<bool>(1);
        std::thread::spawn(move || {
            let success = execute(exe, args, line_tx);
            let _ = done_tx.send_blocking(success);
        });

        cx.spawn(async move |weak, cx| {
            while let Ok(line) = line_rx.recv().await {
                weak.update(cx, |this, cx| this.push_log(line, cx)).ok();
            }
            let success = done_rx.recv().await.unwrap_or(false);
            weak.update(cx, |this, cx| {
                this.busy = false;
                this.status = SharedString::from(if success { "完成" } else { "失败" });
                if success
                    && let Some((dir, name, config)) = save
                    && let Err(error) = instance::write_launch_config(&dir, &name, &config)
                {
                    let text = format!("保存实例设置失败: {error}");
                    this.push_log(text, cx);
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
        let (dir, params) = self.effective_launch(name, cx);
        if dir.is_empty() {
            self.status = SharedString::from("请先设置游戏目录");
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

        self.status = SharedString::from(format!("启动 {name}"));
        self.run_task(args, false, None, cx);
    }

    fn open_folder(&mut self, name: &str, cx: &mut Context<Self>) {
        let dir = Self::value(&self.dir_input, cx);
        let path = Path::new(&dir).join("versions").join(name);
        if !path.is_dir() {
            self.status = SharedString::from("实例目录不存在");
            cx.notify();
            return;
        }
        match Command::new("explorer").arg(path).spawn() {
            Ok(_) => self.status = SharedString::from("已打开实例目录"),
            Err(error) => self.status = SharedString::from(format!("打开目录失败: {error}")),
        }
        cx.notify();
    }

    fn open_install(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let username = Self::value(&self.username_input, cx);
        Self::set_input(&self.install_username_input, &username, window, cx);
        let memory = Self::value(&self.memory_input, cx);
        Self::set_input(&self.install_memory_input, &memory, window, cx);
        let java = Self::value(&self.java_input, cx);
        Self::set_input(&self.install_java_input, &java, window, cx);
        let mirror = Self::value(&self.mirror_input, cx);
        Self::set_input(&self.install_mirror_input, &mirror, window, cx);
        let maven = Self::value(&self.maven_input, cx);
        Self::set_input(&self.install_maven_input, &maven, window, cx);
        self.install_isolate = self.isolate;
        self.instance_page = 1;
        cx.notify();
    }

    fn back_to_instances(&mut self, cx: &mut Context<Self>) {
        self.instance_page = 0;
        cx.notify();
    }

    fn install_version(&mut self, cx: &mut Context<Self>) {
        let dir = Self::value(&self.dir_input, cx);
        if dir.is_empty() {
            self.status = SharedString::from("请先设置游戏目录");
            cx.notify();
            return;
        }
        let version = Self::value(&self.install_version_input, cx);
        if version.is_empty() {
            self.status = SharedString::from("请填写实例版本");
            cx.notify();
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
            version,
        ];
        push_option(&mut args, "--name", &name);
        push_option(&mut args, "--loader", &loader);
        push_option(&mut args, "--loader-version", &loader_version);
        push_option(&mut args, "--java", &java);
        push_option(&mut args, "--mirror", &mirror);
        push_option(&mut args, "--maven", &maven);

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
        self.run_task(args, true, save, cx);
    }

    fn import_pack(&mut self, cx: &mut Context<Self>) {
        let dir = Self::value(&self.dir_input, cx);
        if dir.is_empty() {
            self.status = SharedString::from("请先设置游戏目录");
            cx.notify();
            return;
        }
        let file = Self::value(&self.import_file_input, cx);
        if file.is_empty() {
            self.status = SharedString::from("请填写整合包文件路径");
            cx.notify();
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

        self.run_task(args, true, None, cx);
    }

    fn search_mods(&mut self, cx: &mut Context<Self>) {
        let query = Self::value(&self.mod_query_input, cx);
        let project_type = Self::value(&self.mod_type_input, cx);
        if query.is_empty() || project_type.is_empty() {
            self.status = SharedString::from("请填写搜索关键词与项目类型");
            cx.notify();
            return;
        }
        let limit = Self::value(&self.mod_limit_input, cx);
        if limit.is_empty() {
            self.status = SharedString::from("数量不能为空");
            cx.notify();
            return;
        }
        let mut args = vec![
            String::from("search"),
            String::from("--query"),
            query,
            String::from("--type"),
            project_type,
            String::from("--limit"),
            limit,
        ];
        let loader = Self::value(&self.mod_loader_input, cx);
        push_option(&mut args, "--loader", &loader);
        let version = Self::value(&self.mod_version_input, cx);
        push_option(&mut args, "--version", &version);

        self.run_task(args, false, None, cx);
    }

    fn add_mod(&mut self, cx: &mut Context<Self>) {
        let dir = Self::value(&self.dir_input, cx);
        let name = Self::value(&self.mod_instance_input, cx);
        let slug = Self::value(&self.mod_slug_input, cx);
        let project_type = Self::value(&self.mod_type_input, cx);
        if dir.is_empty() || name.is_empty() || slug.is_empty() || project_type.is_empty() {
            self.status = SharedString::from("请填写游戏目录、实例名、项目和类型");
            cx.notify();
            return;
        }
        let args = vec![
            String::from("add"),
            String::from("--dir"),
            dir,
            String::from("--name"),
            name,
            String::from("--slug"),
            slug,
            String::from("--type"),
            project_type,
        ];
        self.run_task(args, false, None, cx);
    }

    fn open_instance_settings(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        let dir = Self::value(&self.dir_input, cx);
        if dir.is_empty() {
            self.status = SharedString::from("请先设置游戏目录");
            cx.notify();
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
            self.status = SharedString::from("实例或游戏目录为空");
            cx.notify();
            return;
        }
        if Self::value(&self.cfg_memory_input, cx).is_empty() {
            self.status = SharedString::from("内存不能为空");
            cx.notify();
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
        match instance::write_launch_config(&dir, &name, &config) {
            Ok(()) => self.status = SharedString::from("实例设置已保存"),
            Err(error) => self.status = SharedString::from(error),
        }
        self.reload(cx);
    }

    fn save_settings(&mut self, cx: &mut Context<Self>) {
        let memory = self.memory_value(&self.memory_input, cx);
        if Self::value(&self.memory_input, cx).is_empty() {
            self.status = SharedString::from("内存不能为空");
            cx.notify();
            return;
        }
        let value = json!({
            "game_dir": Self::value(&self.dir_input, cx),
            "username": Self::value(&self.username_input, cx),
            "memory": memory.unwrap_or(4096),
            "java": Self::value(&self.java_input, cx),
            "mirror": Self::value(&self.mirror_input, cx),
            "maven": Self::value(&self.maven_input, cx),
            "isolate": self.isolate,
        });
        let path = config_path();
        match write_json(&path, &value) {
            Ok(()) => self.status = SharedString::from("全局设置已保存"),
            Err(error) => self.status = SharedString::from(error),
        }
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

fn execute(exe: PathBuf, args: Vec<String>, tx: channel::Sender<String>) -> bool {
    let child = Command::new(exe)
        .args(&args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();
    let Ok(mut child) = child else {
        let _ = tx.send_blocking(String::from("启动任务失败"));
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

    let status = child.wait();
    match status {
        Ok(status) => {
            let success = status.success();
            let _ = tx.send_blocking(format!("任务退出: {status}"));
            success
        }
        Err(error) => {
            let _ = tx.send_blocking(format!("等待任务失败: {error}"));
            false
        }
    }
}

fn pump<R: Read + Send + 'static>(reader: R, tx: channel::Sender<String>) {
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
                if !line.is_empty() && tx.send_blocking(line.clone()).is_err() {
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
    }
}

fn config_path() -> PathBuf {
    crate::loader::base_dir()
        .join("lyys_mc_launcher")
        .join("config.json")
}

fn write_json(path: &Path, value: &Value) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("创建目录失败: {e}"))?;
    }
    let text = serde_json::to_string_pretty(value).map_err(|e| e.to_string())?;
    std::fs::write(path, text).map_err(|e| format!("写入文件失败: {e}"))
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

/// 标题栏品牌标记：白色圆角块里的几何 L。
fn brand_mark() -> Div {
    div()
        .w(px(26.))
        .h(px(26.))
        .rounded(px(7.))
        .bg(skin::c(skin::APP_ICON))
        .flex()
        .items_center()
        .justify_center()
        .child(
            v_flex()
                .items_start()
                .child(div().w(px(4.)).h(px(12.)).bg(skin::c(skin::ACCENT_FG)))
                .child(div().w(px(10.)).h(px(4.)).bg(skin::c(skin::ACCENT_FG))),
        )
}

fn dot(color: u32, size: f32) -> Div {
    div()
        .w(px(size))
        .h(px(size))
        .rounded_full()
        .flex_shrink_0()
        .bg(skin::c(color))
}

/// 卡片容器：设置页里的分组单元。
fn panel() -> Div {
    div()
        .w_full()
        .flex()
        .flex_col()
        .rounded(px(12.))
        .bg(skin::c(skin::CARD))
        .overflow_hidden()
}

/// 卡片内一行：左侧说明，右侧控件。
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

fn sep() -> Div {
    div().h(px(1.)).mx(px(20.)).bg(skin::c(skin::DIVIDER))
}

fn page_title(title: &'static str, subtitle: &'static str) -> Div {
    v_flex()
        .gap(px(4.))
        .child(
            div()
                .text_size(px(20.))
                .font_semibold()
                .text_color(skin::c(skin::TEXT))
                .child(title),
        )
        .child(
            div()
                .text_size(px(13.))
                .text_color(skin::c(skin::MUTED))
                .child(subtitle),
        )
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

impl LauncherUi {
    fn render_titlebar(&self, _cx: &mut Context<Self>) -> impl IntoElement {
        let status_color = if self.busy { skin::BUSY } else { skin::OK };

        TitleBar::new()
            .bg(skin::c(skin::WINDOW))
            .border_b_1()
            .border_color(skin::c(skin::TITLEBAR_LINE))
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .justify_between()
                    .pr_3()
                    .child(
                        h_flex()
                            .items_center()
                            .gap(px(10.))
                            .child(brand_mark())
                            .child(
                                v_flex()
                                    .gap(px(1.))
                                    .child(
                                        div()
                                            .text_size(px(13.5))
                                            .font_semibold()
                                            .text_color(skin::c(skin::TEXT))
                                            .child("LYYS"),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(8.5))
                                            .font_medium()
                                            .text_color(skin::c(skin::MUTED))
                                            .child("MINECRAFT LAUNCHER"),
                                    ),
                            )
                            .child(
                                div()
                                    .w(px(1.))
                                    .h(px(18.))
                                    .mx(px(4.))
                                    .bg(skin::c(skin::TITLEBAR_LINE)),
                            )
                            .child(
                                div()
                                    .text_size(px(11.5))
                                    .font_medium()
                                    .text_color(skin::c(skin::MUTED))
                                    .child(self.page_label()),
                            ),
                    )
                    .child(
                        h_flex()
                            .items_center()
                            .gap(px(7.))
                            .child(dot(status_color, 6.))
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(skin::c(skin::MUTED))
                                    .child(self.status.clone()),
                            ),
                    ),
            )
    }

    fn page_label(&self) -> SharedString {
        match self.instance_page {
            1 => SharedString::from("安装新实例"),
            2 if !self.cfg_name.is_empty() => {
                SharedString::from(format!("实例设置 · {}", self.cfg_name))
            }
            2 => SharedString::from("实例设置"),
            _ => match self.tab {
                0 => SharedString::from("实例"),
                1 => SharedString::from("内容"),
                _ => SharedString::from("设置"),
            },
        }
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
            .child(
                h_flex().mt(px(14.)).px(px(10.)).pb(px(6.)).child(
                    div()
                        .text_size(px(13.))
                        .text_color(skin::c(skin::MUTED))
                        .child("桌面"),
                ),
            )
            .child(self.side_nav(
                "nav-instances",
                IconName::LayoutDashboard,
                "实例",
                self.tab == 0,
                0,
                cx,
            ))
            .child(self.side_nav(
                "nav-content",
                IconName::SquareTerminal,
                "内容",
                self.tab == 1,
                1,
                cx,
            ))
            .child(
                h_flex().mt(px(18.)).px(px(10.)).pb(px(6.)).child(
                    div()
                        .text_size(px(13.))
                        .text_color(skin::c(skin::MUTED))
                        .child("系统"),
                ),
            )
            .child(self.side_nav(
                "nav-settings",
                IconName::Settings,
                "设置",
                self.tab == 2,
                2,
                cx,
            ))
            .child(div().flex_1())
            .child(
                v_flex()
                    .px(px(10.))
                    .gap(px(2.))
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(skin::c(skin::FAINT))
                            .child("Minecraft · Java 版"),
                    )
                    .child(
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
        label: &'static str,
        active: bool,
        index: usize,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
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
                this.content_tab = 0;
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
            .child(
                h_flex()
                    .w_full()
                    .justify_center()
                    .px(px(40.))
                    .pt(px(10.))
                    .pb(px(18.))
                    .child(v_flex().w_full().max_w(px(820.)).child(self.render_log(cx))),
            )
    }

    fn render_page(&mut self, cx: &mut Context<Self>) -> AnyElement {
        match self.instance_page {
            1 => self.render_install_page(cx).into_any_element(),
            2 => self.render_instance_settings_page(cx).into_any_element(),
            _ => match self.tab {
                0 => self.render_instances(cx).into_any_element(),
                1 => self.render_content(cx).into_any_element(),
                _ => self.render_general_settings(cx).into_any_element(),
            },
        }
    }

    fn render_instances(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let disabled = self.busy;
        let mut list = panel();

        if self.instances.is_empty() {
            list = list.child(
                h_flex().w_full().justify_center().py(px(40.)).child(
                    div()
                        .text_size(px(13.))
                        .text_color(skin::c(skin::FAINT))
                        .child("暂无实例，先安装一个版本或在内容页导入整合包"),
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
                meta.push_str(" · 隔离");
            }
            if index > 0 {
                list = list.child(sep());
            }
            list =
                list.child(
                    row().child(row_text(row_data.name.clone(), meta)).child(
                        h_flex()
                            .flex_shrink_0()
                            .items_center()
                            .gap(px(4.))
                            .child(
                                primary_button(
                                    SharedString::from(format!("launch-{launch_name}")),
                                    "启动",
                                )
                                .when(disabled, |this| this.opacity(0.45))
                                .when(!disabled, |this| {
                                    this.on_click(cx.listener(move |this, _, _, cx| {
                                        this.launch_instance(&launch_click, cx)
                                    }))
                                }),
                            )
                            .child(
                                text_button(
                                    SharedString::from(format!("folder-{launch_name}")),
                                    "目录",
                                )
                                .on_click(cx.listener(
                                    move |this, _, _, cx| this.open_folder(&folder_name, cx),
                                )),
                            )
                            .child(
                                text_button(
                                    SharedString::from(format!("settings-{launch_name}")),
                                    "设置",
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
                    .child(page_title("实例", "管理和启动本机已安装的游戏实例"))
                    .child(
                        h_flex()
                            .flex_shrink_0()
                            .items_center()
                            .gap(px(6.))
                            .pt(px(4.))
                            .child(text_button("refresh", "刷新").when(!disabled, |this| {
                                this.on_click(cx.listener(|this, _, _, cx| this.reload(cx)))
                            }))
                            .child(primary_button("open-install", "安装新实例").on_click(
                                cx.listener(|this, _, window, cx| this.open_install(window, cx)),
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

    fn render_content(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let pack_active = self.content_tab == 0;

        let mut body = v_flex().gap(px(16.));
        if pack_active {
            body = body
                .child(
                    panel()
                        .child(
                            row()
                                .child(row_text("包文件路径", "支持 .zip 与 .mrpack 整合包文件"))
                                .child(self.input_control(&self.import_file_input)),
                        )
                        .child(sep())
                        .child(
                            row()
                                .child(row_text("实例名称", "留空时使用整合包内的名称"))
                                .child(self.input_control(&self.import_name_input)),
                        )
                        .child(sep())
                        .child(
                            row()
                                .child(row_text("导入可选模组", "同时安装作者标记为可选的模组"))
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
                    primary_button("import-run", "导入整合包").when(!self.busy, |this| {
                        this.on_click(cx.listener(|this, _, _, cx| this.import_pack(cx)))
                    }),
                ));
        } else {
            body = body
                .child(
                    panel()
                        .child(
                            row()
                                .child(row_text("关键词", "搜索 Modrinth 上的模组、整合包与资源包"))
                                .child(self.input_control(&self.mod_query_input)),
                        )
                        .child(sep())
                        .child(
                            row()
                                .child(row_text("类型", "mod / modpack / resourcepack / shader"))
                                .child(self.input_control(&self.mod_type_input)),
                        )
                        .child(sep())
                        .child(
                            row()
                                .child(row_text("加载器", "例如 fabric、forge、neoforge"))
                                .child(self.input_control(&self.mod_loader_input)),
                        )
                        .child(sep())
                        .child(
                            row()
                                .child(row_text("游戏版本", "例如 1.20.1，留空表示不限"))
                                .child(self.input_control(&self.mod_version_input)),
                        )
                        .child(sep())
                        .child(
                            row()
                                .child(row_text("数量", "一次返回的结果条数"))
                                .child(self.input_control(&self.mod_limit_input)),
                        ),
                )
                .child(h_flex().w_full().justify_end().child(
                    primary_button("search-run", "搜索").when(!self.busy, |this| {
                        this.on_click(cx.listener(|this, _, _, cx| this.search_mods(cx)))
                    }),
                ))
                .child(
                    panel()
                        .child(
                            row()
                                .child(row_text("项目 slug", "Modrinth 项目页地址中的标识"))
                                .child(self.input_control(&self.mod_slug_input)),
                        )
                        .child(sep())
                        .child(
                            row()
                                .child(row_text("实例名称", "把资源安装到这个实例"))
                                .child(self.input_control(&self.mod_instance_input)),
                        ),
                )
                .child(h_flex().w_full().justify_end().child(
                    primary_button("add-run", "添加到实例").when(!self.busy, |this| {
                        this.on_click(cx.listener(|this, _, _, cx| this.add_mod(cx)))
                    }),
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
                    .child(page_title("内容", "导入整合包、检索并安装 Modrinth 资源"))
                    .child(
                        h_flex()
                            .flex_shrink_0()
                            .gap(px(2.))
                            .p(px(3.))
                            .rounded(px(9.))
                            .bg(skin::c(skin::PANEL))
                            .child(self.segment("tab-pack", "整合包", pack_active, 0, cx))
                            .child(self.segment("tab-mods", "模组", !pack_active, 1, cx)),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .id("content-scroll")
                    .overflow_y_scroll()
                    .pb(px(6.))
                    .child(body),
            )
    }

    fn segment(
        &self,
        id: &'static str,
        label: &'static str,
        active: bool,
        index: usize,
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
                this.content_tab = index;
                cx.notify();
            }))
            .child(label)
    }

    fn render_general_settings(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .size_full()
            .gap(px(20.))
            .child(page_title("设置", "全局启动偏好，对所有实例生效"))
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
                                            .child(row_text(
                                                "游戏目录",
                                                "Minecraft 安装位置，实例从 versions 读取",
                                            ))
                                            .child(self.input_control(&self.dir_input)),
                                    )
                                    .child(sep())
                                    .child(
                                        row()
                                            .child(row_text("用户名", "启动游戏时使用的玩家名称"))
                                            .child(self.input_control(&self.username_input)),
                                    )
                                    .child(sep())
                                    .child(
                                        row()
                                            .child(row_text("内存 (MB)", "分配给游戏的最大堆内存"))
                                            .child(self.input_control(&self.memory_input)),
                                    ),
                            )
                            .child(
                                panel()
                                    .child(
                                        row()
                                            .child(row_text("Java 路径", "留空时自动查找可用 Java"))
                                            .child(self.input_control(&self.java_input)),
                                    )
                                    .child(sep())
                                    .child(
                                        row()
                                            .child(row_text(
                                                "镜像地址",
                                                "替换资源下载源，留空表示官方源",
                                            ))
                                            .child(self.input_control(&self.mirror_input)),
                                    )
                                    .child(sep())
                                    .child(
                                        row()
                                            .child(row_text("Maven 仓库", "自定义依赖仓库地址"))
                                            .child(self.input_control(&self.maven_input)),
                                    ),
                            )
                            .child(
                                panel().child(
                                    row()
                                        .child(row_text(
                                            "默认隔离运行新实例",
                                            "每个实例使用独立目录，避免模组互相污染",
                                        ))
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
                                    primary_button("save-settings", "保存全局设置").on_click(
                                        cx.listener(|this, _, _, cx| this.save_settings(cx)),
                                    ),
                                ),
                            ),
                    ),
            )
    }

    fn render_install_page(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
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
                            .child(text_button("install-back", "←").on_click(
                                cx.listener(|this, _, _, cx| this.back_to_instances(cx)),
                            )),
                    )
                    .child(page_title("安装新实例", "选择版本与加载器，创建独立实例")),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .id("install-scroll")
                    .overflow_y_scroll()
                    .pb(px(6.))
                    .child(
                        v_flex()
                            .gap(px(16.))
                            .child(
                                panel()
                                    .child(
                                        row()
                                            .child(row_text("MC 版本", "例如 1.20.1"))
                                            .child(self.input_control(&self.install_version_input)),
                                    )
                                    .child(sep())
                                    .child(
                                        row()
                                            .child(row_text("实例名称", "留空则使用版本号命名"))
                                            .child(self.input_control(&self.install_name_input)),
                                    )
                                    .child(sep())
                                    .child(
                                        row()
                                            .child(row_text(
                                                "加载器",
                                                "fabric / forge / neoforge / quilt",
                                            ))
                                            .child(self.input_control(&self.install_loader_input)),
                                    )
                                    .child(sep())
                                    .child(
                                        row()
                                            .child(row_text("加载器版本", "留空时使用最新版本"))
                                            .child(
                                                self.input_control(
                                                    &self.install_loader_version_input,
                                                ),
                                            ),
                                    ),
                            )
                            .child(
                                panel()
                                    .child(
                                        row()
                                            .child(row_text("用户名", "启动游戏时使用的玩家名称"))
                                            .child(
                                                self.input_control(&self.install_username_input),
                                            ),
                                    )
                                    .child(sep())
                                    .child(
                                        row()
                                            .child(row_text("内存 (MB)", "分配给游戏的最大堆内存"))
                                            .child(self.input_control(&self.install_memory_input)),
                                    )
                                    .child(sep())
                                    .child(
                                        row()
                                            .child(row_text("Java 路径", "留空时自动查找可用 Java"))
                                            .child(self.input_control(&self.install_java_input)),
                                    )
                                    .child(sep())
                                    .child(
                                        row()
                                            .child(row_text("镜像地址", "替换资源下载源"))
                                            .child(self.input_control(&self.install_mirror_input)),
                                    )
                                    .child(sep())
                                    .child(
                                        row()
                                            .child(row_text("Maven 仓库", "自定义依赖仓库地址"))
                                            .child(self.input_control(&self.install_maven_input)),
                                    ),
                            )
                            .child(
                                panel().child(
                                    row()
                                        .child(row_text(
                                            "隔离运行",
                                            "实例使用独立目录，模组与存档互不影响",
                                        ))
                                        .child(
                                            Switch::new("install-isolate")
                                                .checked(self.install_isolate)
                                                .on_click(cx.listener(
                                                    |this, checked: &bool, _, cx| {
                                                        this.install_isolate = *checked;
                                                        cx.notify();
                                                    },
                                                )),
                                        ),
                                ),
                            )
                            .child(h_flex().w_full().justify_end().child(
                                primary_button("install-run", "开始安装").when(
                                    !self.busy,
                                    |this| {
                                        this.on_click(
                                            cx.listener(|this, _, _, cx| this.install_version(cx)),
                                        )
                                    },
                                ),
                            )),
                    ),
            )
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
                    .child(page_title("实例设置", "这里的修改只对当前实例生效")),
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
                                        row().child(row_text("实例", self.cfg_name.clone())).child(
                                            div()
                                                .text_size(px(12.5))
                                                .text_color(skin::c(skin::MUTED))
                                                .child("覆盖全局设置"),
                                        ),
                                    )
                                    .child(sep())
                                    .child(
                                        row()
                                            .child(row_text("用户名", "启动游戏时使用的玩家名称"))
                                            .child(self.input_control(&self.cfg_username_input)),
                                    )
                                    .child(sep())
                                    .child(
                                        row()
                                            .child(row_text("内存 (MB)", "分配给游戏的最大堆内存"))
                                            .child(self.input_control(&self.cfg_memory_input)),
                                    )
                                    .child(sep())
                                    .child(
                                        row()
                                            .child(row_text("Java 路径", "留空时使用全局设置"))
                                            .child(self.input_control(&self.cfg_java_input)),
                                    )
                                    .child(sep())
                                    .child(
                                        row()
                                            .child(row_text("镜像地址", "留空时使用全局设置"))
                                            .child(self.input_control(&self.cfg_mirror_input)),
                                    )
                                    .child(sep())
                                    .child(
                                        row()
                                            .child(row_text("Maven 仓库", "留空时使用全局设置"))
                                            .child(self.input_control(&self.cfg_maven_input)),
                                    ),
                            )
                            .child(
                                panel().child(
                                    row()
                                        .child(row_text(
                                            "隔离运行",
                                            "实例使用独立目录，模组与存档互不影响",
                                        ))
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
                                primary_button("cfg-save", "保存实例设置").on_click(
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

    fn render_log(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let mono = cx.theme().mono_font_family.clone();
        let summary = self.log_summary.clone();
        let status_color = if self.busy { skin::BUSY } else { skin::FAINT };

        if self.log_follow {
            self.log_follow = false;
            self.log_scroll.scroll_to_bottom();
        }

        v_flex()
            .w_full()
            .rounded(px(12.))
            .bg(skin::c(skin::CARD))
            .overflow_hidden()
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .justify_between()
                    .gap(px(16.))
                    .px(px(20.))
                    .py(px(10.))
                    .child(
                        h_flex()
                            .min_w_0()
                            .flex_1()
                            .items_center()
                            .gap(px(8.))
                            .child(dot(status_color, 6.))
                            .child(
                                div()
                                    .flex_shrink_0()
                                    .text_size(px(12.5))
                                    .text_color(skin::c(skin::TEXT))
                                    .child("日志"),
                            )
                            .child(
                                div()
                                    .min_w_0()
                                    .flex_1()
                                    .truncate()
                                    .text_size(px(12.))
                                    .text_color(skin::c(skin::FAINT))
                                    .child(summary),
                            ),
                    )
                    .child(
                        text_button("log-toggle", if self.log_open { "收起" } else { "展开" })
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.log_open = !this.log_open;
                                this.log_follow = true;
                                cx.notify();
                            })),
                    ),
            )
            .when(self.log_open, |panel| {
                panel.child(sep()).child(
                    div()
                        .id("log-scroll")
                        .h(px(160.))
                        .px(px(20.))
                        .py(px(12.))
                        .overflow_y_scroll()
                        .track_scroll(&self.log_scroll)
                        .font_family(mono)
                        .text_size(px(11.5))
                        .text_color(skin::c(skin::MUTED))
                        .child(v_flex().gap(px(3.)).children(if self.log_text.is_empty() {
                            vec![SharedString::from("尚未运行任务")]
                        } else {
                            self.log_text
                                .lines()
                                .map(|line| SharedString::from(line.to_string()))
                                .collect::<Vec<_>>()
                        })),
                )
            })
    }
}
