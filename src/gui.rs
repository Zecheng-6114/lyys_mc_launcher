use std::error::Error;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use gpui::{
    AnyElement, App, AppContext, Bounds, Context, CursorStyle, Entity, IntoElement, ParentElement,
    Render, ScrollHandle, SharedString, Styled, Window, WindowBounds, WindowOptions, div,
    prelude::*, px, rgb, size,
};
use gpui_component::{
    ActiveTheme, Disableable, Icon, IconName, Root, Sizable, StyledExt, TitleBar,
    button::{Button, ButtonVariants},
    checkbox::Checkbox,
    h_flex, input,
    theme::{Theme, ThemeMode},
    v_flex,
};
use gpui_kit_assets::Assets;
use gpui_platform::application;
use serde_json::{Value, json};
use smol::channel;

use crate::instance::{self, LaunchConfig};

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
        Theme::sync_system_appearance(None, cx);

        let bounds = Bounds::centered(None, size(px(1120.), px(780.)), cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(TitleBar::title_bar_options()),
            ..Default::default()
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
        let theme = cx.theme();
        let background = theme.background;
        let foreground = theme.foreground;

        v_flex()
            .size_full()
            .bg(background)
            .text_color(foreground)
            .text_size(px(13.))
            .child(
                TitleBar::new().child(
                    h_flex()
                        .w_full()
                        .justify_between()
                        .child(
                            div()
                                .text_size(px(14.))
                                .font_semibold()
                                .child("lyys_mc_launcher"),
                        )
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(theme.muted_foreground)
                                .child(self.status.clone()),
                        ),
                ),
            )
            .child(self.render_body(cx))
            .child(self.render_log(cx))
    }
}

impl LauncherUi {
    fn render_body(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        div()
            .flex_1()
            .flex()
            .flex_row()
            .min_w_0()
            .min_h_0()
            .bg(theme.background)
            .child(self.render_nav(cx))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .flex()
                    .flex_col()
                    .p(px(18.))
                    .gap_3()
                    .child(self.render_page(cx)),
            )
    }

    fn render_nav(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let status_color = if self.busy {
            cx.theme().info
        } else {
            cx.theme().success
        };

        div()
            .w(px(64.))
            .h_full()
            .flex()
            .flex_col()
            .items_center()
            .pt(px(16.))
            .pb(px(16.))
            .gap_1()
            .bg(cx.theme().sidebar)
            .border_r_1()
            .border_color(cx.theme().sidebar_border)
            .child(self.nav_item(
                "nav-instances",
                IconName::LayoutDashboard,
                "实例",
                self.tab == 0,
                0,
                cx,
            ))
            .child(self.nav_item(
                "nav-content",
                IconName::SquareTerminal,
                "内容",
                self.tab == 1,
                1,
                cx,
            ))
            .child(self.nav_item(
                "nav-settings",
                IconName::Settings,
                "设置",
                self.tab == 2,
                2,
                cx,
            ))
            .child(div().flex_1())
            .child(div().w(px(10.)).h(px(10.)).rounded_full().bg(status_color))
    }

    #[allow(clippy::too_many_arguments)]
    fn nav_item(
        &self,
        id: &'static str,
        icon: IconName,
        label: &'static str,
        active: bool,
        index: usize,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = cx.theme();
        let (bg, fg) = if active {
            (theme.sidebar_accent, theme.sidebar_accent_foreground)
        } else {
            (theme.transparent, theme.sidebar_foreground)
        };
        let hover_bg = if active { bg } else { theme.sidebar_accent };

        v_flex()
            .id(id)
            .w(px(48.))
            .h(px(48.))
            .items_center()
            .justify_center()
            .rounded(px(12.))
            .bg(bg)
            .text_color(fg)
            .hover(move |style| style.bg(hover_bg))
            .cursor(CursorStyle::PointingHand)
            .on_click(cx.listener(move |this, _event, _window, cx| {
                this.tab = index;
                this.instance_page = 0;
                this.content_tab = 0;
                cx.notify();
            }))
            .child(Icon::new(icon).with_size(px(20.)).text_color(fg))
            .child(
                div()
                    .h(px(3.))
                    .text_size(px(10.))
                    .text_color(fg)
                    .child(label),
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
}

const VERSION_TAG_LIGHT: u32 = 0xdbeafe;
const VERSION_TAG_DARK: u32 = 0x1e3a8a;
const VERSION_TAG_LIGHT_FG: u32 = 0x1d4ed8;
const VERSION_TAG_DARK_FG: u32 = 0x93c5fd;
const LOADER_TAG_LIGHT: u32 = 0xede9fe;
const LOADER_TAG_DARK: u32 = 0x3b0764;
const LOADER_TAG_LIGHT_FG: u32 = 0x6d28d9;
const LOADER_TAG_DARK_FG: u32 = 0xc4b5fd;
const ISOLATE_TAG_LIGHT: u32 = 0xd1fae5;
const ISOLATE_TAG_DARK: u32 = 0x064e3b;
const ISOLATE_TAG_LIGHT_FG: u32 = 0x047857;
const ISOLATE_TAG_DARK_FG: u32 = 0x6ee7b7;

impl LauncherUi {
    fn render_instances(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let dark = theme.mode == ThemeMode::Dark;
        let muted = theme.muted_foreground;
        let border = theme.border;
        let list = theme.colors.list;

        let version_style = if dark {
            (rgb(VERSION_TAG_DARK), rgb(VERSION_TAG_DARK_FG))
        } else {
            (rgb(VERSION_TAG_LIGHT), rgb(VERSION_TAG_LIGHT_FG))
        };
        let loader_style = if dark {
            (rgb(LOADER_TAG_DARK), rgb(LOADER_TAG_DARK_FG))
        } else {
            (rgb(LOADER_TAG_LIGHT), rgb(LOADER_TAG_LIGHT_FG))
        };
        let isolate_style = if dark {
            (rgb(ISOLATE_TAG_DARK), rgb(ISOLATE_TAG_DARK_FG))
        } else {
            (rgb(ISOLATE_TAG_LIGHT), rgb(ISOLATE_TAG_LIGHT_FG))
        };

        let mut rows = v_flex().gap_2();
        if self.instances.is_empty() {
            rows = rows.child(
                div()
                    .py(px(32.))
                    .text_color(muted)
                    .child("暂无实例，可在安装页创建或在内容页导入整合包"),
            );
        }
        for row in &self.instances {
            let launch_name = row.name.clone().to_string();
            let folder_name = row.name.clone();
            let settings_name = row.name.clone();
            let disabled = self.busy;

            rows = rows.child(
                h_flex()
                    .items_center()
                    .gap_3()
                    .px_4()
                    .py(px(10.))
                    .rounded(px(10.))
                    .border_1()
                    .border_color(border)
                    .bg(list)
                    .child(
                        div().min_w_0().flex_1().child(
                            v_flex()
                                .gap_1()
                                .child(
                                    div()
                                        .text_size(px(14.))
                                        .font_semibold()
                                        .truncate()
                                        .child(row.name.clone()),
                                )
                                .child(
                                    h_flex()
                                        .gap_2()
                                        .items_center()
                                        .child(
                                            h_flex()
                                                .rounded(px(5.))
                                                .px_2()
                                                .py(px(1.))
                                                .bg(version_style.0)
                                                .text_color(version_style.1)
                                                .text_size(px(11.))
                                                .child(row.version.clone()),
                                        )
                                        .child(
                                            h_flex()
                                                .rounded(px(5.))
                                                .px_2()
                                                .py(px(1.))
                                                .bg(loader_style.0)
                                                .text_color(loader_style.1)
                                                .text_size(px(11.))
                                                .child(row.loader.clone()),
                                        )
                                        .when(row.isolated, |row| {
                                            row.child(
                                                h_flex()
                                                    .rounded(px(5.))
                                                    .px_2()
                                                    .py(px(1.))
                                                    .bg(isolate_style.0)
                                                    .text_color(isolate_style.1)
                                                    .text_size(px(11.))
                                                    .child("隔离"),
                                            )
                                        }),
                                ),
                        ),
                    )
                    .child(
                        h_flex()
                            .gap_1()
                            .child(
                                Button::new(SharedString::from(format!("launch-{launch_name}")))
                                    .primary()
                                    .small()
                                    .label("启动")
                                    .disabled(disabled)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.launch_instance(&launch_name, cx)
                                    })),
                            )
                            .child(
                                Button::new(SharedString::from(format!("folder-{folder_name}")))
                                    .ghost()
                                    .small()
                                    .label("目录")
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.open_folder(&folder_name, cx)
                                    })),
                            )
                            .child(
                                Button::new(SharedString::from(format!(
                                    "settings-{settings_name}"
                                )))
                                .ghost()
                                .small()
                                .label("设置")
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
            .gap_3()
            .child(
                h_flex()
                    .items_center()
                    .justify_between()
                    .child(div().text_size(px(18.)).font_semibold().child("实例"))
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                Button::new("refresh")
                                    .ghost()
                                    .small()
                                    .label("刷新")
                                    .disabled(self.busy)
                                    .on_click(cx.listener(|this, _, _, cx| this.reload(cx))),
                            )
                            .child(
                                Button::new("open-install")
                                    .primary()
                                    .small()
                                    .label("安装新实例")
                                    .disabled(self.busy)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.open_install(window, cx)
                                    })),
                            ),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .id("instance-list")
                    .overflow_y_scroll()
                    .child(rows),
            )
    }

    fn render_content(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let pack_active = self.content_tab == 0;
        let mod_active = self.content_tab == 1;

        let mut body = v_flex().gap_3().pt_2();
        if pack_active {
            body = body
                .child(self.field_row("包文件路径", &self.import_file_input, cx))
                .child(self.field_row("实例名称", &self.import_name_input, cx))
                .child(
                    Checkbox::new("import-optional")
                        .label("导入可选模组")
                        .checked(self.import_optional)
                        .on_click(cx.listener(|this, checked: &bool, _, cx| {
                            this.import_optional = *checked;
                            cx.notify();
                        })),
                )
                .child(
                    h_flex().justify_end().child(
                        Button::new("import-run")
                            .primary()
                            .label("导入整合包")
                            .disabled(self.busy)
                            .on_click(cx.listener(|this, _, _, cx| this.import_pack(cx))),
                    ),
                );
        }
        if mod_active {
            body = body
                .child(
                    div()
                        .text_color(muted)
                        .child("搜索 Modrinth 上的模组等资源"),
                )
                .child(self.field_row("关键词", &self.mod_query_input, cx))
                .child(self.field_row("类型", &self.mod_type_input, cx))
                .child(self.field_row("加载器", &self.mod_loader_input, cx))
                .child(self.field_row("游戏版本", &self.mod_version_input, cx))
                .child(self.field_row("数量", &self.mod_limit_input, cx))
                .child(
                    h_flex().justify_end().child(
                        Button::new("search-run")
                            .primary()
                            .label("搜索")
                            .disabled(self.busy)
                            .on_click(cx.listener(|this, _, _, cx| this.search_mods(cx))),
                    ),
                )
                .child(div().h(px(1.)).bg(theme.border))
                .child(div().text_color(muted).child("把项目安装到实例"))
                .child(self.field_row("项目 slug", &self.mod_slug_input, cx))
                .child(self.field_row("实例名称", &self.mod_instance_input, cx))
                .child(
                    h_flex().justify_end().child(
                        Button::new("add-run")
                            .primary()
                            .label("添加到实例")
                            .disabled(self.busy)
                            .on_click(cx.listener(|this, _, _, cx| this.add_mod(cx))),
                    ),
                );
        }

        v_flex()
            .size_full()
            .gap_3()
            .child(
                h_flex()
                    .gap_1()
                    .items_center()
                    .child(
                        Button::new("tab-pack")
                            .label("整合包")
                            .when(pack_active, |button| button.primary())
                            .when(!pack_active, |button| button.ghost())
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.content_tab = 0;
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("tab-mods")
                            .label("模组")
                            .when(mod_active, |button| button.primary())
                            .when(!mod_active, |button| button.ghost())
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.content_tab = 1;
                                cx.notify();
                            })),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .id("content-scroll")
                    .overflow_y_scroll()
                    .child(body),
            )
    }

    fn render_general_settings(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();

        v_flex()
            .size_full()
            .gap_3()
            .child(div().text_size(px(18.)).font_semibold().child("设置"))
            .child(
                v_flex()
                    .gap_3()
                    .p_4()
                    .rounded(px(12.))
                    .border_1()
                    .border_color(theme.border)
                    .bg(theme.colors.list)
                    .child(self.field_row("游戏目录", &self.dir_input, cx))
                    .child(self.field_row("用户名", &self.username_input, cx))
                    .child(self.field_row("内存 (MB)", &self.memory_input, cx))
                    .child(self.field_row("Java 路径", &self.java_input, cx))
                    .child(self.field_row("镜像地址", &self.mirror_input, cx))
                    .child(self.field_row("Maven 仓库", &self.maven_input, cx))
                    .child(
                        Checkbox::new("settings-isolate")
                            .label("默认隔离运行新实例")
                            .checked(self.isolate)
                            .on_click(cx.listener(|this, checked: &bool, _, cx| {
                                this.isolate = *checked;
                                cx.notify();
                            })),
                    )
                    .child(
                        h_flex().justify_end().child(
                            Button::new("save-settings")
                                .primary()
                                .label("保存全局设置")
                                .on_click(cx.listener(|this, _, _, cx| this.save_settings(cx))),
                        ),
                    ),
            )
    }

    fn render_install_page(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();

        v_flex()
            .size_full()
            .gap_3()
            .child(
                h_flex()
                    .items_center()
                    .justify_between()
                    .child(div().text_size(px(18.)).font_semibold().child("安装新实例"))
                    .child(
                        Button::new("install-back")
                            .ghost()
                            .small()
                            .label("返回")
                            .on_click(cx.listener(|this, _, _, cx| this.back_to_instances(cx))),
                    ),
            )
            .child(
                v_flex()
                    .gap_3()
                    .p_4()
                    .rounded(px(12.))
                    .border_1()
                    .border_color(theme.border)
                    .bg(theme.colors.list)
                    .child(self.field_row("MC 版本", &self.install_version_input, cx))
                    .child(self.field_row("实例名称", &self.install_name_input, cx))
                    .child(self.field_row("加载器", &self.install_loader_input, cx))
                    .child(self.field_row("加载器版本", &self.install_loader_version_input, cx))
                    .child(self.field_row("用户名", &self.install_username_input, cx))
                    .child(self.field_row("内存 (MB)", &self.install_memory_input, cx))
                    .child(self.field_row("Java 路径", &self.install_java_input, cx))
                    .child(self.field_row("镜像地址", &self.install_mirror_input, cx))
                    .child(self.field_row("Maven 仓库", &self.install_maven_input, cx))
                    .child(
                        Checkbox::new("install-isolate")
                            .label("隔离运行")
                            .checked(self.install_isolate)
                            .on_click(cx.listener(|this, checked: &bool, _, cx| {
                                this.install_isolate = *checked;
                                cx.notify();
                            })),
                    )
                    .child(
                        h_flex().justify_end().child(
                            Button::new("install-run")
                                .primary()
                                .label("开始安装")
                                .disabled(self.busy)
                                .on_click(cx.listener(|this, _, _, cx| this.install_version(cx))),
                        ),
                    ),
            )
    }

    fn render_instance_settings_page(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();

        v_flex()
            .size_full()
            .gap_3()
            .child(
                h_flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_size(px(18.))
                            .font_semibold()
                            .child(format!("实例设置: {}", self.cfg_name)),
                    )
                    .child(
                        Button::new("cfg-back")
                            .ghost()
                            .small()
                            .label("返回")
                            .on_click(cx.listener(|this, _, _, cx| this.back_to_instances(cx))),
                    ),
            )
            .child(
                v_flex()
                    .gap_3()
                    .p_4()
                    .rounded(px(12.))
                    .border_1()
                    .border_color(theme.border)
                    .bg(theme.colors.list)
                    .child(self.field_row("用户名", &self.cfg_username_input, cx))
                    .child(self.field_row("内存 (MB)", &self.cfg_memory_input, cx))
                    .child(self.field_row("Java 路径", &self.cfg_java_input, cx))
                    .child(self.field_row("镜像地址", &self.cfg_mirror_input, cx))
                    .child(self.field_row("Maven 仓库", &self.cfg_maven_input, cx))
                    .child(
                        Checkbox::new("cfg-isolate")
                            .label("隔离运行")
                            .checked(self.cfg_isolate)
                            .on_click(cx.listener(|this, checked: &bool, _, cx| {
                                this.cfg_isolate = *checked;
                                cx.notify();
                            })),
                    )
                    .child(
                        h_flex().justify_end().child(
                            Button::new("cfg-save")
                                .primary()
                                .label("保存实例设置")
                                .on_click(
                                    cx.listener(|this, _, _, cx| this.save_instance_settings(cx)),
                                ),
                        ),
                    ),
            )
    }

    fn field_row(
        &self,
        label: &'static str,
        state: &Entity<input::InputState>,
        cx: &Context<Self>,
    ) -> impl IntoElement {
        h_flex()
            .items_center()
            .gap_3()
            .child(
                div()
                    .w(px(110.))
                    .flex_shrink_0()
                    .text_color(cx.theme().muted_foreground)
                    .child(label),
            )
            .child(input::Input::new(state).flex_1().small())
    }

    fn render_log(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let summary = self.log_summary.clone();

        if self.log_follow {
            self.log_follow = false;
            self.log_scroll.scroll_to_bottom();
        }

        v_flex()
            .mx_2()
            .mb_2()
            .rounded(px(10.))
            .border_1()
            .border_color(theme.border)
            .bg(theme.colors.list)
            .overflow_hidden()
            .child(
                h_flex()
                    .items_center()
                    .justify_between()
                    .px_3()
                    .py(px(6.))
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .truncate()
                            .text_size(px(12.))
                            .text_color(theme.muted_foreground)
                            .child(if summary.is_empty() {
                                SharedString::from("日志")
                            } else {
                                summary
                            }),
                    )
                    .child(
                        Button::new("log-toggle")
                            .ghost()
                            .xsmall()
                            .label(if self.log_open {
                                "收起日志"
                            } else {
                                "展开日志"
                            })
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.log_open = !this.log_open;
                                this.log_follow = true;
                                cx.notify();
                            })),
                    ),
            )
            .when(self.log_open, |panel| {
                panel.child(
                    div()
                        .id("log-scroll")
                        .h(px(200.))
                        .overflow_y_scroll()
                        .track_scroll(&self.log_scroll)
                        .p_3()
                        .font_family(theme.mono_font_family.clone())
                        .text_size(px(11.))
                        .child(
                            div()
                                .flex_col()
                                .gap_1()
                                .children(if self.log_text.is_empty() {
                                    vec![SharedString::from("尚未运行任务")]
                                } else {
                                    self.log_text
                                        .lines()
                                        .map(|line| SharedString::from(line.to_string()))
                                        .collect::<Vec<_>>()
                                }),
                        ),
                )
            })
    }
}
