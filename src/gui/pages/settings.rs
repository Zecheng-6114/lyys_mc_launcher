use eframe::egui;

use crate::core::config::{self, InstanceConfig};
use crate::core::i18n::{self, Language, t, tf};
use crate::gui::app::App;

impl App {
    pub(crate) fn settings_page(&mut self, ui: &mut egui::Ui) {
        ui.heading(t("Settings"));
        let path = config::path()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|_| t("unavailable").to_owned());
        ui.label(
            egui::RichText::new(tf("stored in {}", &[path.as_str()]))
                .weak()
                .small(),
        );
        egui::ScrollArea::vertical()
            .id_salt("settings")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(t("Language:"));
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
                ui.group(|ui| {
                    ui.label(egui::RichText::new(t("Global configuration")).strong());
                    ui.horizontal(|ui| {
                        ui.label(t("Minecraft dir:"));
                        ui.add(
                            egui::TextEdit::singleline(&mut self.minecraft_dir)
                                .id_salt("settings_minecraft_dir"),
                        );
                    });
                    ui.horizontal(|ui| {
                        ui.label(t("Installed Java"));
                        if ui
                            .add_enabled(!self.java_scanning, egui::Button::new(t("Scan")))
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
                        .radio(selected.is_empty(), t("auto (match the game version)"))
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
                    ui.checkbox(&mut self.isolate, t("Version isolation"));
                });
                ui.add_space(10.0);
                ui.label(egui::RichText::new(t("Instances")).strong());
                ui.label(
                    egui::RichText::new(t(
                        "Each instance uses the global configuration unless overridden below",
                    ))
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
                        egui::RichText::new(t("no installed instance found"))
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
                            .checkbox(
                                &mut use_global,
                                tf("{}: use global configuration", &[id.as_str()]),
                            )
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
                        ui.checkbox(&mut entry.isolate, t("Version isolation"));
                        ui.horizontal(|ui| {
                            ui.label(t("Java path:"));
                            ui.add(
                                egui::TextEdit::singleline(&mut entry.java_path)
                                    .id_salt(("instance_java", id.as_str()))
                                    .hint_text(t("inherit global")),
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
                if ui.button(t("Save configuration")).clicked() {
                    self.save_config();
                }
                if let Some(status) = &self.config_status {
                    ui.label(egui::RichText::new(status).weak().small());
                }
            });
    }
}
