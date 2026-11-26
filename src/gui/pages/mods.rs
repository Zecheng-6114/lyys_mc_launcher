use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use eframe::egui;

use mc_launcher_core::loader::LoaderKind;

use crate::core::config;
use crate::core::i18n::{t, tf};
use crate::core::logging;
use crate::core::modpack::{self, InstallOptions, PackProgress};
use crate::core::modrinth::{self, ModSearchRequest, ProjectType};
use crate::gui::app::{
    App, MOD_MANIFEST_FILE, Message, ModAction, ModRecord,
};
use crate::gui::components::buttons::mod_result;

impl App {
    pub(crate) fn mods_page(&mut self, ui: &mut egui::Ui) {
        ui.label(egui::RichText::new(t("content.powered_by")).weak());
        let can_switch_type = self.mod_installing.is_none();
        egui::Grid::new("content_filters")
            .num_columns(2)
            .spacing([10.0, 8.0])
            .show(ui, |ui| {
                ui.label(t("content.type"));
                ui.horizontal_wrapped(|ui| {
                    ui.add_enabled_ui(can_switch_type, |ui| {
                        for kind in [
                            ProjectType::Mod,
                            ProjectType::ResourcePack,
                            ProjectType::ShaderPack,
                            ProjectType::DataPack,
                            ProjectType::Modpack,
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
                ui.end_row();

                ui.label(t("content.query"));
                ui.horizontal(|ui| {
                    let response = ui.add(
                        egui::TextEdit::singleline(&mut self.mod_query)
                            .id_salt("mod_query")
                            .hint_text(t("content.query_hint"))
                            .desired_width(220.0),
                    );
                    let submitted = response.lost_focus()
                        && ui.input(|input| input.key_pressed(egui::Key::Enter));
                    if ui
                        .add_enabled(!self.mod_searching, egui::Button::new(t("common.search")))
                        .clicked()
                        || submitted
                    {
                        self.start_mod_search(ui.ctx());
                    }
                    if self.mod_searching {
                        ui.add(egui::widgets::Spinner::new().size(14.0));
                    }
                });
                ui.end_row();

                ui.label(t("content.minecraft"));
                ui.horizontal_wrapped(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.mod_minecraft_version)
                            .id_salt("mod_minecraft_version")
                            .hint_text(t("common.any_lower"))
                            .desired_width(100.0),
                    );
                    if self.mod_kind == ProjectType::Mod || self.mod_kind == ProjectType::Modpack {
                        ui.label(t("install.loader"));
                        egui::ComboBox::from_id_salt("mod_loader")
                            .width(110.0)
                            .selected_text(
                                self.mod_loader.map(loader_label).unwrap_or(t("common.any")),
                            )
                            .show_ui(ui, |ui| {
                                ui.selectable_value(&mut self.mod_loader, None, t("common.any"));
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
                        ui.label(t("content.world"));
                        let selected = if self.mod_datapack_world.is_empty() {
                            t("common.pick").to_owned()
                        } else {
                            self.mod_datapack_world.clone()
                        };
                        egui::ComboBox::from_id_salt("datapack_world")
                            .width(160.0)
                            .selected_text(selected)
                            .show_ui(ui, |ui| {
                                if self.datapack_worlds.is_empty() {
                                    ui.label(t("content.world_none"));
                                }
                                for world in &self.datapack_worlds {
                                    ui.selectable_value(
                                        &mut self.mod_datapack_world,
                                        world.clone(),
                                        world,
                                    );
                                }
                            });
                    }
                    if !self.installed.is_empty() && self.mod_kind != ProjectType::Modpack {
                        ui.label(t("content.from_instance"));
                        let mut picked: Option<(String, Option<String>, Option<&'static str>)> = None;
                        egui::ComboBox::from_id_salt("mod_instance")
                            .width(220.0)
                            .selected_text(
                                self.mod_instance_id.as_deref().unwrap_or(t("common.pick")),
                            )
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
                                            self.mod_instance_id.as_deref()
                                                == Some(instance.id.as_str()),
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
                ui.end_row();
            });
        if let Some(error) = &self.mod_error {
            ui.label(tf("error.prefix", &[("message", error.as_str())]));
        }
        if let Some(notice) = &self.mod_notice {
            ui.label(notice);
        }
        if let Some(progress) = &self.mod_progress {
            ui.label(egui::RichText::new(progress).weak());
        }
        let game_dir = self.game_dir();
        let target_dir = self.content_dir(self.mod_kind);
        if self.mod_kind == ProjectType::Modpack {
            self.modpack_form(ui);
        } else {
            if self.mod_kind == ProjectType::DataPack
                && self.datapack_worlds_dir != game_dir.display().to_string()
            {
                self.scan_worlds(&game_dir);
            }
            if self.installed_mods_dir != target_dir.display().to_string()
                || self.installed_kind != Some(self.mod_kind)
            {
                self.load_installed_mods(&target_dir, self.mod_kind);
                self.start_update_check(ui.ctx(), &target_dir, self.mod_kind);
            }
            let isolated_suffix = if self.mods_isolate() {
                t("content.isolated_suffix")
            } else {
                ""
            };
            let target = target_dir.display().to_string();
            ui.label(
                egui::RichText::new(tf(
                    "content.target",
                    &[("dir", target.as_str()), ("suffix", isolated_suffix)],
                ))
                    .weak()
                    .small(),
            );
        }
        if self.mod_results.is_empty() {
            if !self.mod_searching && self.mod_error.is_none() {
                ui.label(t("content.search_hint"));
            }
        } else {
            ui.horizontal(|ui| {
                let total = self.mod_total.to_string();
                let shown = self.mod_results.len().to_string();
                ui.label(tf(
                    "content.results_showing",
                    &[("total", total.as_str()), ("shown", shown.as_str())],
                ));
                if ui.small_button(t("content.reload_icons")).clicked() {
                    ui.ctx().forget_all_images();
                }
            });
            let target_ready = self.mod_kind == ProjectType::Modpack
                || !self.mods_isolate()
                || self.mods_target_instance().is_some();
            let world_ready = self.mod_kind != ProjectType::DataPack
                || !self.mod_datapack_world.trim().is_empty();
            let needs_loader = self.mod_kind == ProjectType::Mod;
            let filters_ready = target_ready
                && world_ready
                && !self.mod_minecraft_version.trim().is_empty()
                && (!needs_loader || self.mod_loader.is_some());
            if !target_ready {
                ui.label(
                    egui::RichText::new(t("content.need_instance_for_content"))
                        .weak()
                        .small(),
                );
            } else if !world_ready {
                ui.label(
                    egui::RichText::new(t("content.pick_world"))
                        .weak()
                        .small(),
                );
            } else if !filters_ready {
                let hint = if needs_loader {
                    t("content.need_version_loader")
                } else {
                    t("content.need_version")
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
                        let loader = if self.mod_kind == ProjectType::Mod
                            || self.mod_kind == ProjectType::Modpack
                        {
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
                        egui::Button::new(t("common.previous")),
                    )
                    .clicked()
                {
                    self.goto_mod_page(self.mod_page - 1, ui.ctx());
                }
                let page = self.mod_page.to_string();
                let pages = total_pages.to_string();
                ui.label(tf("content.page", &[("page", page.as_str()), ("pages", pages.as_str())]));
                if ui
                    .add_enabled(
                        self.mod_page < total_pages && !self.mod_searching,
                        egui::Button::new(t("common.next")),
                    )
                    .clicked()
                {
                    self.goto_mod_page(self.mod_page + 1, ui.ctx());
                }
            });
        }
    }

    /// The instance whose mods directory should receive downloads when isolation is on.
    pub(crate) fn mods_target_instance(&self) -> Option<&str> {
        self.mod_instance_id.as_deref().or_else(|| {
            let launch = self.launch_version.trim();
            (!launch.is_empty()).then_some(launch)
        })
    }

    /// Isolation that applies to the current mods target.
    pub(crate) fn mods_isolate(&self) -> bool {
        self.mods_target_instance()
            .map_or(self.isolate, |instance| self.effective_isolate(instance))
    }

    /// Game directory: `versions/<instance>` when version isolation is on.
    pub(crate) fn game_dir(&self) -> PathBuf {
        let base = self.minecraft_dir();
        match self.mods_target_instance() {
            Some(instance) if self.mods_isolate() => base.join("versions").join(instance),
            _ => base,
        }
    }

    /// Directory that receives downloads of `kind`.
    pub(crate) fn content_dir(&self, kind: ProjectType) -> PathBuf {
        match kind {
            ProjectType::Mod => self.game_dir().join("mods"),
            ProjectType::ResourcePack => self.game_dir().join("resourcepacks"),
            ProjectType::ShaderPack => self.game_dir().join("shaderpacks"),
            ProjectType::DataPack => self
                .game_dir()
                .join("saves")
                .join(self.mod_datapack_world.trim())
                .join("datapacks"),
            ProjectType::Modpack => self.game_dir(),
        }
    }

    /// Scan `saves/` for worlds that can receive data packs.
    pub(crate) fn scan_worlds(&mut self, game_dir: &Path) {
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

    pub(crate) fn manifest_file(kind: ProjectType) -> &'static str {
        match kind {
            ProjectType::Mod => MOD_MANIFEST_FILE,
            _ => ".lyys_content.json",
        }
    }

    /// Read the manifest for `dir`, dropping entries whose file is gone.
    pub(crate) fn load_installed_mods(&mut self, dir: &Path, kind: ProjectType) {
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
    pub(crate) fn start_update_check(&mut self, ctx: &egui::Context, dir: &Path, kind: ProjectType) {
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

    pub(crate) fn uninstall_mod(&mut self, key: &str, dir: &Path, kind: ProjectType) {
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
                logging::error("content", &error.to_string());
                self.mod_error = Some(tf(
                    "error.content_remove",
                    &[("name", record.filename.as_str()), ("error", error.to_string().as_str())],
                ))
            }
        }
        removed += self.remove_unused_dependencies(&record.dependencies, dir);
        self.save_installed_mods(dir, kind);
        if self.mod_error.is_none() {
            let removed = removed.to_string();
            self.mod_notice = Some(tf(
                "content.uninstalled",
                &[("name", record.filename.as_str()), ("count", removed.as_str())],
            ));
        }
    }

    /// Remove auto-installed dependencies that are no longer required by another mod.
    pub(crate) fn remove_unused_dependencies(&mut self, dependencies: &[String], dir: &Path) -> usize {
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

    pub(crate) fn dependency_in_use(&self, key: &str) -> bool {
        self.installed_mods.values().any(|record| {
            record
                .dependencies
                .iter()
                .any(|dependency| dependency == key)
        })
    }

    pub(crate) fn save_installed_mods(&mut self, dir: &Path, kind: ProjectType) {
        match serde_json::to_string_pretty(&self.installed_mods) {
            Ok(text) => {
                if let Err(error) = std::fs::write(dir.join(Self::manifest_file(kind)), text) {
                    let error = error.to_string();
                    self.log
                        .push(tf("error.manifest_write", &[("error", error.as_str())]));
                }
            }
            Err(error) => {
                let error = error.to_string();
                self.log
                    .push(tf("error.manifest_encode", &[("error", error.as_str())]));
            }
        }
    }

    pub(crate) fn start_modpack_install(&mut self, ctx: &egui::Context) {
        if self.modpack_installing || self.mod_installing.is_some() {
            return;
        }
        let path = PathBuf::from(self.modpack_file.trim());
        if !path.is_file() {
            self.modpack_error = Some(t("modpack.error.file_not_found").to_owned());
            return;
        }
        self.modpack_installing = true;
        self.modpack_notice = None;
        self.modpack_error = None;
        self.modpack_progress = Some(t("modpack.installing").to_owned());
        let dir = self.minecraft_dir();
        let java = if self.java_path.trim().is_empty() {
            PathBuf::from("java")
        } else {
            PathBuf::from(self.java_path.trim())
        };
        let with_optional = self.modpack_with_optional;
        let mirror = self.config.download_mirror.trim().to_owned();
        let tx = self.tx.clone();
        let progress_tx = tx.clone();
        let progress_ctx = ctx.clone();
        let done_ctx = ctx.clone();
        let done_dir = dir.clone();
        self.handle.spawn(async move {
            let options = InstallOptions {
                minecraft_dir: &dir,
                java: &java,
                mirror: &mirror,
                with_optional,
            };
            let reporter = Arc::new(move |progress: PackProgress| {
                let _ = progress_tx.send(Message::ModpackProgress(progress.describe()));
                progress_ctx.request_repaint();
            });
            let result = modpack::install_file(&path, &options, reporter)
                .await
                .map_err(|error| error.to_string());
            let _ = tx.send(Message::ModpackDone {
                dir: done_dir,
                result,
            });
            done_ctx.request_repaint();
        });
    }

    pub(crate) fn start_mod_search(&mut self, ctx: &egui::Context) {
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
    pub(crate) fn request_mod_page(&mut self, page: u32, ctx: &egui::Context) {
        if self.mod_searching {
            return;
        }
        self.mod_searching = true;
        self.mod_error = None;
        let query = self.mod_query.trim().to_owned();
        let minecraft_version = self.mod_minecraft_version.trim().to_owned();
        let kind = self.mod_kind;
        let loader = if kind == ProjectType::Mod || kind == ProjectType::Modpack {
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
            .map_err(|error| {
                logging::error("modrinth", &error.to_string());
                error.message()
            });
            let _ = tx.send(Message::ModSearch { page, result });
            ctx.request_repaint();
        });
    }

    pub(crate) fn goto_mod_page(&mut self, page: u32, ctx: &egui::Context) {
        if page == 0 || page == self.mod_page {
            return;
        }
        self.request_mod_page(page, ctx);
    }

    /// 把当前搜索结果里的图标下载到本地缓存，已缓存的跳过；每张成功后刷新界面。
    pub(crate) fn cache_result_icons(&mut self, ctx: &egui::Context) {
        let mut urls: Vec<String> = Vec::new();
        for hit in &self.mod_results {
            let Some(url) = &hit.icon_url else {
                continue;
            };
            if modrinth::icon_cache_path(url).is_some_and(|path| path.is_file()) {
                continue;
            }
            if !urls.iter().any(|cached| cached == url) {
                urls.push(url.clone());
            }
        }
        if urls.is_empty() {
            return;
        }
        let ctx = ctx.clone();
        self.handle.spawn(async move {
            for url in urls {
                if modrinth::cache_icon(&url).await.is_ok() {
                    ctx.request_repaint();
                }
            }
        });
    }

    pub(crate) fn start_mod_install(
        &mut self,
        project: String,
        game_version: String,
        loader: Option<String>,
        kind: ProjectType,
        ctx: &egui::Context,
    ) {
        if game_version.trim().is_empty() {
            self.mod_error = Some(t("content.need_minecraft_version").to_owned());
            return;
        }
        if kind == ProjectType::Mod && loader.as_deref().unwrap_or("").trim().is_empty() {
            self.mod_error = Some(t("content.need_loader").to_owned());
            return;
        }
        if kind == ProjectType::DataPack && self.mod_datapack_world.trim().is_empty() {
            self.mod_error = Some(t("content.need_world").to_owned());
            return;
        }
        if kind != ProjectType::Modpack
            && self.mods_isolate()
            && self.mods_target_instance().is_none()
        {
            self.mod_error =
                Some(t("content.need_instance").to_owned());
            return;
        }
        self.mod_installing = Some(project.clone());
        self.mod_progress = Some(t("content.downloading").to_owned());
        self.mod_error = None;
        self.mod_notice = None;
        if kind == ProjectType::Modpack {
            // 与本地安装共用同一套进度展示
            self.modpack_installing = true;
            self.modpack_progress = Some(t("modpack.installing").to_owned());
            self.modpack_notice = None;
            self.modpack_error = None;
            let minecraft_dir = self.minecraft_dir();
            let java = self
                .effective_java("")
                .or_else(|| {
                    self.java_versions
                        .first()
                        .map(|runtime| runtime.executable.clone())
                })
                .unwrap_or_else(|| PathBuf::from("java"));
            let loader = loader.unwrap_or_default();
            let tx = self.tx.clone();
            let progress_tx = self.tx.clone();
            let ctx = ctx.clone();
            let progress_ctx = ctx.clone();
            let archive_tx = self.tx.clone();
            let archive_ctx = ctx.clone();
            let done_dir = minecraft_dir.clone();
            let mirror = self.config.download_mirror.trim().to_owned();
            self.handle.spawn(async move {
                // 在线安装同样用真实进度回调，而不是空回调
                let reporter = Arc::new(move |progress: PackProgress| {
                    let _ = progress_tx.send(Message::ModpackProgress(progress.describe()));
                    progress_ctx.request_repaint();
                });
                let result = install_modpack_from_modrinth(
                    &project,
                    game_version.trim(),
                    &loader,
                    &minecraft_dir,
                    &java,
                    &mirror,
                    &mut move |progress: modrinth::ContentProgress| {
                        let _ = archive_tx.send(Message::ModpackProgress(progress.describe()));
                        archive_ctx.request_repaint();
                    },
                    reporter,
                )
                .await;
                let _ = tx.send(Message::ModpackDone {
                    dir: done_dir,
                    result: result.map_err(|error| error.to_string()),
                });
                ctx.request_repaint();
            });
            return;
        }
        let target = self.content_dir(kind);
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
                        &mut |progress: modrinth::ContentProgress| {
                            let _ = tx.send(Message::ModProgress(progress.describe()));
                            ctx.request_repaint();
                        },
                    )
                    .await
                }
                _ => {
                    modrinth::install_content(
                        &project,
                        game_version.trim(),
                        &target,
                        &mut |progress: modrinth::ContentProgress| {
                            let _ = tx.send(Message::ModProgress(progress.describe()));
                            ctx.request_repaint();
                        },
                    )
                    .await
                }
            }
            .map_err(|error| {
                logging::error("content", &error.to_string());
                error.message()
            });
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
}

/// Download a modpack from Modrinth and install it as a new instance.
async fn install_modpack_from_modrinth(
    project: &str,
    game_version: &str,
    loader: &str,
    minecraft_dir: &Path,
    java: &Path,
    mirror: &str,
    archive_progress: &mut (dyn FnMut(modrinth::ContentProgress) + Send),
    reporter: Arc<dyn Fn(PackProgress) + Send + Sync>,
) -> Result<modpack::InstallReport, String> {
    let cache = config::cache_dir()
        .map(|dir| dir.join("modpacks"))
        .unwrap_or_else(|_| std::env::temp_dir().join("lyys-modpacks"));
    let archive = cache.join(format!("{}.mrpack", safe_file_name(project)));
    let loader = (!loader.trim().is_empty()).then_some(loader);
    modrinth::download_primary_file(project, game_version, loader, &archive, archive_progress)
        .await
        .map_err(|error| error.to_string())?;
    let options = InstallOptions {
        minecraft_dir,
        java,
        mirror,
        with_optional: false,
    };
    let result = modpack::install_file(&archive, &options, reporter).await;
    let _ = std::fs::remove_file(&archive);
    result.map_err(|error| error.to_string())
}

fn safe_file_name(name: &str) -> String {
    name.chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '-' || character == '_' {
                character
            } else {
                '_'
            }
        })
        .collect()
}

fn content_label(kind: ProjectType) -> &'static str {
    match kind {
        ProjectType::Mod => t("content.type.mods"),
        ProjectType::ResourcePack => t("content.type.resourcepacks"),
        ProjectType::ShaderPack => t("content.type.shaders"),
        ProjectType::DataPack => t("content.type.datapacks"),
        ProjectType::Modpack => t("content.type.modpacks"),
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

fn loader_kind(tag: &str) -> Option<LoaderKind> {
    match tag {
        "Fabric" => Some(LoaderKind::Fabric),
        "Quilt" => Some(LoaderKind::Quilt),
        "Forge" => Some(LoaderKind::Forge),
        "NeoForge" => Some(LoaderKind::NeoForge),
        _ => None,
    }
}
