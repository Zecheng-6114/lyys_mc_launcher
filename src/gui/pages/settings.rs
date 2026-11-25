use eframe::egui;

use crate::core::config::{self, InstanceConfig};
use crate::core::i18n::{self, Language, t, tf};
use crate::gui::app::{App, DirPickTarget};

impl App {
    pub(crate) fn settings_page(&mut self, ui: &mut egui::Ui) {
        let path = config::path()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|_| t("common.unavailable").to_owned());
        ui.label(
            egui::RichText::new(tf("settings.stored_in", &[("path", path.as_str())]))
                .weak()
                .small(),
        );
        egui::ScrollArea::vertical()
            .id_salt("settings")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(t("settings.language"));
                    let mut language = i18n::current();
                    egui::ComboBox::from_id_salt("settings_language")
                        .selected_text(language.label())
                        .show_ui(ui, |ui| {
                            for choice in Language::ALL {
                                ui.selectable_value(&mut language, choice, choice.label());
                            }
                        });
                    if language != i18n::current() {
                        i18n::set_language(language);
                        self.save_config();
                    }
                });
                ui.add_space(10.0);
                self.settings_global_group(ui);
                ui.add_space(10.0);
                self.settings_dirs_group(ui);
                ui.add_space(10.0);
                self.settings_instances_group(ui);
                ui.add_space(10.0);
                if ui.button(t("settings.save")).clicked() {
                    self.save_config();
                }
                if let Some(status) = &self.config_status {
                    ui.label(egui::RichText::new(status).weak().small());
                }
            });
    }

    /// 全局 Java 与版本隔离设置。
    fn settings_global_group(&mut self, ui: &mut egui::Ui) {
        ui.group(|ui| {
            ui.label(egui::RichText::new(t("settings.global")).strong());
            ui.horizontal(|ui| {
                ui.label(t("settings.java.installed"));
                if ui
                    .add_enabled(!self.java_scanning, egui::Button::new(t("common.scan")))
                    .clicked()
                {
                    self.scan_java(ui.ctx().clone());
                }
                if self.java_scanning {
                    ui.spinner();
                }
            });
            let selected = self.java_path.trim().to_owned();
            let mut chosen: Option<String> = None;
            if ui
                .radio(selected.is_empty(), t("settings.java.auto"))
                .clicked()
            {
                chosen = Some(String::new());
            }
            for runtime in &self.java_versions {
                let path = runtime.executable.display().to_string();
                let label = format!("Java {}  {}", runtime.version, path);
                if ui.radio(selected == path, label).clicked() {
                    chosen = Some(path);
                }
            }
            if let Some(path) = chosen {
                self.java_path = path;
                self.persist_java();
            }
            // 开关类改动和目录操作一样立即保存，文本输入交给“保存配置”
            let isolate_changed = ui.checkbox(&mut self.isolate, t("settings.isolate")).changed();
            if isolate_changed {
                self.save_config();
            }
        });
    }

    /// Minecraft 目录列表：添加、命名、切换与移除。
    fn settings_dirs_group(&mut self, ui: &mut egui::Ui) {
        let active_index = self.config.active_index();
        let count = self.config.minecraft_dirs.len();
        let picking = self.dir_picking;
        let mut activate: Option<usize> = None;
        let mut remove: Option<usize> = None;
        let mut browse: Option<usize> = None;
        let mut browse_new = false;
        let mut add: Option<String> = None;

        ui.group(|ui| {
            ui.label(egui::RichText::new(t("settings.minecraft_dirs")).strong());
            ui.label(
                egui::RichText::new(t("settings.minecraft_dirs_hint"))
                    .weak()
                    .small(),
            );
            if count == 0 {
                ui.label(egui::RichText::new(t("settings.dir_empty")).weak().small());
            }
            // 选择框打开期间锁住列表，避免结果回来时对不上号
            ui.add_enabled_ui(!picking, |ui| {
                for (index, dir) in self.config.minecraft_dirs.iter_mut().enumerate() {
                    // 用分隔线区分每一项，避免外层分组里再套分组
                    if index > 0 {
                        ui.separator();
                    }
                    ui.horizontal_wrapped(|ui| {
                        if ui
                            .radio(Some(index) == active_index, t("settings.dir_active"))
                            .clicked()
                        {
                            activate = Some(index);
                        }
                        ui.add(
                            egui::TextEdit::singleline(&mut dir.name)
                                .id_salt(("dir_name", index))
                                .hint_text(t("settings.dir_name_hint"))
                                .desired_width(110.0),
                        );
                        if ui.button(t("settings.browse")).clicked() {
                            browse = Some(index);
                        }
                        if ui
                            .add_enabled(count > 1, egui::Button::new(t("common.remove")))
                            .clicked()
                        {
                            remove = Some(index);
                        }
                    });
                    ui.add(
                        egui::TextEdit::singleline(&mut dir.path)
                            .id_salt(("dir_path", index))
                            .hint_text(t("settings.dir_path_hint"))
                            .desired_width(f32::INFINITY),
                    );
                }
                ui.horizontal_wrapped(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.new_dir_path)
                            .id_salt("new_minecraft_dir")
                            .hint_text(t("settings.dir_path_hint"))
                            .desired_width(240.0),
                    );
                    if ui.button(t("settings.browse")).clicked() {
                        browse_new = true;
                    }
                    if ui
                        .add_enabled(
                            !self.new_dir_path.trim().is_empty(),
                            egui::Button::new(t("settings.dir_add")),
                        )
                        .clicked()
                    {
                        add = Some(self.new_dir_path.trim().to_owned());
                    }
                });
            });
        });

        // 直接编辑当前目录的路径时，让“当前目录”跟着这一行走，
        // 否则路径一改就匹配不上，活动目录会意外跳到第一个
        if activate.is_none() && remove.is_none() && add.is_none() {
            if let Some(index) = active_index {
                if let Some(path) = self
                    .config
                    .minecraft_dirs
                    .get(index)
                    .map(|dir| dir.path.trim().to_owned())
                {
                    if !path.is_empty() {
                        self.config.active_minecraft_dir = path;
                    }
                }
            }
        }

        if let Some(index) = browse {
            self.request_dir_pick(DirPickTarget::Existing(index), ui.ctx());
        }
        if browse_new {
            self.request_dir_pick(DirPickTarget::NewDir, ui.ctx());
        }
        if let Some(index) = activate {
            if let Some(path) = self
                .config
                .minecraft_dirs
                .get(index)
                .map(|dir| dir.path.trim().to_owned())
            {
                if !path.is_empty() {
                    self.use_dir(&path);
                }
            }
        }
        if let Some(index) = remove {
            self.remove_dir(index);
        }
        if let Some(path) = add {
            self.use_dir(&path);
            self.new_dir_path.clear();
        }
    }

    /// 当前目录下的实例覆盖设置。
    fn settings_instances_group(&mut self, ui: &mut egui::Ui) {
        let active_label = self
            .config
            .active_dir()
            .map(|dir| {
                let name = dir.name.trim();
                if name.is_empty() {
                    dir.path.trim().to_owned()
                } else {
                    name.to_owned()
                }
            })
            .unwrap_or_default();
        ui.label(
            egui::RichText::new(tf(
                "settings.instances_for",
                &[("dir", active_label.as_str())],
            ))
            .strong(),
        );
        ui.label(
            egui::RichText::new(t("settings.instances_hint"))
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
                egui::RichText::new(t("settings.instances_none"))
                    .weak()
                    .small(),
            );
            return;
        }
        let global_isolate = self.isolate;
        let Some(dir) = self.config.active_dir_mut() else {
            return;
        };
        let mut changed = false;
        for id in ids {
            let mut use_global = dir
                .instances
                .get(&id)
                .map(|config| config.use_global)
                .unwrap_or(true);
            let mut toggled = false;
            ui.group(|ui| {
                if ui
                    .checkbox(
                        &mut use_global,
                        tf("settings.instance_use_global", &[("instance", id.as_str())]),
                    )
                    .changed()
                {
                    toggled = true;
                }
                if use_global {
                    return;
                }
                let entry = dir.instances.entry(id.clone()).or_insert_with(|| InstanceConfig {
                    use_global: false,
                    // 取消继承后从全局值起步，方便在此基础上修改
                    isolate: global_isolate,
                    java_path: String::new(),
                });
                ui.checkbox(&mut entry.isolate, t("settings.isolate"));
                ui.horizontal(|ui| {
                    ui.label(t("settings.java_path"));
                    ui.add(
                        egui::TextEdit::singleline(&mut entry.java_path)
                            .id_salt(("instance_java", id.as_str()))
                            .hint_text(t("settings.inherit_global")),
                    );
                });
                let java_path = entry.java_path.trim();
                if !java_path.is_empty() && !std::path::Path::new(java_path).is_file() {
                    ui.colored_label(ui.visuals().error_fg_color, t("settings.java_invalid"));
                }
            });
            if toggled {
                let entry = dir
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
                changed = true;
            }
        }
        // 勾选是离散操作，和目录的切换/增删一样立即保存
        if changed {
            self.save_config();
        }
    }
}
