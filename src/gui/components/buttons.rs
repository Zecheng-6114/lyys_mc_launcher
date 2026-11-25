use eframe::egui;

use crate::core::i18n::{t, tf};
use crate::core::modrinth::{self, ModSearchHit, ProjectType};
use crate::gui::app::ModAction;

pub(crate) fn mod_result(
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
                let image = match modrinth::icon_cache_path(icon_url)
                    .filter(|path| path.is_file())
                {
                    Some(path) => egui::Image::from_uri(file_uri(&path)),
                    None => egui::Image::new(icon_url.as_str()),
                };
                ui.add(image.fit_to_exact_size(egui::vec2(48.0, 48.0)));
            }
            ui.vertical(|ui| {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(&hit.title).strong());
                    let downloads = hit.downloads.to_string();
                    ui.label(
                        egui::RichText::new(tf(
                            "search.downloads",
                            &[("count", downloads.as_str())],
                        ))
                        .weak(),
                    );
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
                        t("content.state.installed").to_owned()
                    } else {
                        tf("content.state.installed_version", &[("version", version)])
                    };
                    ui.label(egui::RichText::new(label).weak().small());
                }
                ui.horizontal(|ui| {
                    if installing {
                        ui.add(egui::widgets::Spinner::new().size(12.0));
                    }
                    if installed {
                        if ui
                            .add_enabled(
                                actions_enabled && target_ready,
                                egui::Button::new(t("common.uninstall")),
                            )
                            .clicked()
                        {
                            action = Some(ModAction::Uninstall);
                        }
                        if ui
                            .add_enabled(
                                actions_enabled && target_ready,
                                egui::Button::new(t("common.reinstall")),
                            )
                            .clicked()
                        {
                            action = Some(ModAction::Reinstall);
                        }
                        if let Some(number) = update_number {
                            if ui
                                .add_enabled(
                                    actions_enabled && target_ready,
                                    egui::Button::new(tf(
                                        "content.update_to",
                                        &[("version", number)],
                                    )),
                                )
                                .clicked()
                            {
                                action = Some(ModAction::Update);
                            }
                        }
                    } else if ui
                        .add_enabled(
                            actions_enabled && target_ready && filters_ready,
                            egui::Button::new(t("common.install")),
                        )
                        .clicked()
                    {
                        action = Some(ModAction::Install);
                    }
                    ui.hyperlink_to(
                        t("content.open_modrinth"),
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

/// 把本地缓存路径转成 egui 文件加载器支持的 `file://` URI。
fn file_uri(path: &std::path::Path) -> String {
    let raw = path.display().to_string();
    if raw.starts_with('/') {
        format!("file://{raw}")
    } else {
        // Windows 绝对路径需要 `file:///C:/...` 形式
        format!("file:///{}", raw.replace('\\', "/"))
    }
}
