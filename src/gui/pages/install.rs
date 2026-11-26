use std::path::PathBuf;

use eframe::egui;

use crate::core::i18n::{t, tf};
use crate::core::install::{self, InstallProgress};
use crate::core::logging;
use crate::gui::app::{App, InstallSection, LoaderChoice, Message, Task};

impl App {
    /// 安装页入口：在「安装实例」与「安装内容」两个区之间切换。
    pub(crate) fn install_page(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.selectable_value(
                &mut self.install_section,
                InstallSection::Instance,
                t("nav.install"),
            );
            ui.selectable_value(
                &mut self.install_section,
                InstallSection::Content,
                t("install.section.content"),
            );
        });
        ui.separator();
        match self.install_section {
            InstallSection::Instance => self.instance_install_page(ui),
            InstallSection::Content => self.mods_page(ui),
        }
    }

    pub(crate) fn instance_install_page(&mut self, ui: &mut egui::Ui) {
        let loader_key = (self.install_loader, self.install_version.trim().to_owned());
        if self.install_loader != LoaderChoice::Vanilla
            && self.loader_versions_key.as_ref() != Some(&loader_key)
            && !self.loader_versions_loading
        {
            self.loader_versions_key = Some(loader_key);
            self.load_loader_versions(ui.ctx());
        }
        // 页面整体可滚动，窗口不高时安装按钮也不会被裁掉
        egui::ScrollArea::vertical()
            .id_salt("install_page")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(t("install.version"));
                    ui.add(
                        egui::TextEdit::singleline(&mut self.install_version)
                            .id_salt("install_version"),
                    );
                    if ui
                        .add_enabled(
                            !self.versions_loading,
                            egui::Button::new(t("common.refresh_list")),
                        )
                        .clicked()
                    {
                        self.reload_versions(ui.ctx());
                    }
                });
                ui.horizontal(|ui| {
                    ui.checkbox(&mut self.show_all_versions, t("install.show_all"));
                    if self.versions_loading {
                        ui.add(egui::widgets::Spinner::new().size(14.0));
                        ui.label(t("install.versions_loading"));
                    } else if let Some(error) = &self.versions_error {
                        ui.label(tf("error.versions_load", &[("error", error.as_str())]));
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
                                        format!(
                                            "{}  ({})",
                                            version.id,
                                            version_kind_label(&version.kind)
                                        ),
                                    )
                                    .clicked()
                                {
                                    self.install_version = version.id.clone();
                                }
                            }
                        });
                }
                ui.horizontal_wrapped(|ui| {
                    ui.label(t("install.loader"));
                    egui::ComboBox::from_id_salt("install_loader")
                        .selected_text(self.install_loader.label())
                        .show_ui(ui, |ui| {
                            for choice in LoaderChoice::ALL {
                                ui.selectable_value(&mut self.install_loader, choice, choice.label());
                            }
                        });
                    if self.install_loader != LoaderChoice::Vanilla {
                        ui.label(t("install.loader_version"));
                        ui.add(
                            egui::TextEdit::singleline(&mut self.install_loader_version)
                                .id_salt("install_loader_version")
                                .hint_text(t("common.latest")),
                        );
                        if ui
                            .add_enabled(
                                !self.loader_versions_loading,
                                egui::Button::new(t("common.refresh")),
                            )
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
                        ui.label(tf("error.loaders_load", &[("error", error.as_str())]));
                    }
                    if self.loader_versions.is_empty() {
                        if !self.loader_versions_loading && self.loader_versions_error.is_none() {
                            ui.label(t("install.loader_none"));
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
                }
                ui.horizontal(|ui| {
                    ui.label(t("settings.minecraft_dir"));
                    self.dir_picker(ui);
                });

                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(!self.installing, egui::Button::new(t("common.install")))
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
                    let downloaded = progress.downloaded.to_string();
                    let skipped = progress.skipped.to_string();
                    ui.monospace(tf(
                        "install.files_summary",
                        &[
                            ("downloaded", downloaded.as_str()),
                            ("skipped", skipped.as_str()),
                        ],
                    ));
                    if let Some(current) = &progress.current {
                        ui.monospace(tf("install.current", &[("name", current.as_str())]));
                    }
                }
            });
    }

    pub(crate) fn reload_versions(&mut self, ctx: &egui::Context) {
        self.versions_loading = true;
        self.versions_error = None;
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        let mirror = self.config.download_mirror.trim().to_owned();
        self.handle.spawn(async move {
            let result = install::available_versions(&mirror)
                .await
                .map_err(|error| error.to_string());
            let _ = tx.send(Message::Versions(result));
            ctx.request_repaint();
        });
    }

    pub(crate) fn reload_loader_versions(&mut self, ctx: &egui::Context) {
        let Some(kind) = self.install_loader.kind() else {
            return;
        };
        self.loader_versions_loading = true;
        self.loader_versions_error = None;
        self.loader_versions.clear();
        let minecraft_version = self.install_version.trim().to_owned();
        let key = (self.install_loader, minecraft_version.clone());
        self.loader_versions_key = Some(key.clone());
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        let mirror = self.config.download_mirror.trim().to_owned();
        self.handle.spawn_blocking(move || {
            let result = install::loader_versions(kind, &minecraft_version, &mirror)
                .map_err(|error| error.to_string());
            let _ = tx.send(Message::LoaderVersions { key, result });
            ctx.request_repaint();
        });
    }

    /// 优先使用本地缓存的加载器版本，没有缓存时才请求网络。
    pub(crate) fn load_loader_versions(&mut self, ctx: &egui::Context) {
        let Some(kind) = self.install_loader.kind() else {
            return;
        };
        let minecraft_version = self.install_version.trim().to_owned();
        if let Some(cached) = install::cached_loader_versions(kind, &minecraft_version) {
            self.loader_versions = cached;
            self.loader_versions_error = None;
            self.loader_versions_loading = false;
            return;
        }
        self.reload_loader_versions(ctx);
    }

    pub(crate) fn start_install(&mut self, ctx: &egui::Context) {
        let version = self.install_version.trim().to_owned();
        if version.is_empty() {
            self.log.push(t("install.need_version").to_owned());
            return;
        }
        self.installing = true;
        self.install_progress = Some(InstallProgress {
            downloaded: 0,
            skipped: 0,
            current: None,
        });
        let dir = self.minecraft_dir();
        let java = if self.java_path.trim().is_empty() {
            PathBuf::from("java")
        } else {
            PathBuf::from(self.java_path.trim())
        };
        let loader_name = self.install_loader.label();
        let loader = self.install_loader.to_spec(&self.install_loader_version);
        let mirror = self.config.download_mirror.trim().to_owned();
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
                        &mirror,
                        &mut reporter,
                    ) {
                        Ok(result) => {
                            logging::info(&format!(
                                "installed {} {} into {}",
                                loader_name,
                                result.version_id,
                                dir.display()
                            ));
                            tf(
                                "install.installed",
                                &[
                                    ("version", loader_name),
                                    ("loader", result.version_id.as_str()),
                                ],
                            )
                        }
                        Err(error) => {
                            let error = error.to_string();
                            logging::error("install", &error);
                            tf("error.install_failed", &[("error", error.as_str())])
                        }
                    }
                }
                None => match install::install_with_reporter(&dir, &version, &mirror, &mut reporter)
                {
                    Ok(_) => {
                        logging::info(&format!(
                            "installed {version} into {}",
                            dir.display()
                        ));
                        tf("install.installed_only", &[("version", version.as_str())])
                    }
                    Err(error) => {
                        let error = error.to_string();
                        logging::error("install", &error);
                        tf("error.install_failed", &[("error", error.as_str())])
                    }
                },
            };
            let _ = tx.send(Message::Log(line));
            let _ = tx.send(Message::TaskDone(Task::Install));
            ctx.request_repaint();
        });
    }
}

/// 版本类型显示名，避免中文界面里直接出现 release / snapshot。
fn version_kind_label(kind: &str) -> &'static str {
    match kind {
        "release" => t("install.kind.release"),
        "snapshot" => t("install.kind.snapshot"),
        "old_beta" => t("install.kind.old_beta"),
        "old_alpha" => t("install.kind.old_alpha"),
        _ => t("install.kind.other"),
    }
}
