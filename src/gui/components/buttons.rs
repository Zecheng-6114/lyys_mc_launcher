use eframe::egui;

use crate::core::i18n::{t, tf};
use crate::core::modrinth::{ModSearchHit, ProjectType};
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
                ui.add(
                    egui::Image::new(icon_url.as_str()).fit_to_exact_size(egui::vec2(48.0, 48.0)),
                );
            }
            ui.vertical(|ui| {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(&hit.title).strong());
                    let downloads = hit.downloads.to_string();
                    ui.label(
                        egui::RichText::new(tf("{} downloads", &[downloads.as_str()])).weak(),
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
                        t("installed").to_owned()
                    } else {
                        tf("installed v{}", &[version])
                    };
                    ui.label(egui::RichText::new(label).weak().small());
                }
                ui.horizontal(|ui| {
                    if installing {
                        ui.add(egui::widgets::Spinner::new().size(12.0));
                    }
                    if installed {
                        if ui
                            .add_enabled(actions_enabled && target_ready, egui::Button::new(t("Uninstall")))
                            .clicked()
                        {
                            action = Some(ModAction::Uninstall);
                        }
                        if ui
                            .add_enabled(
                                actions_enabled && target_ready,
                                egui::Button::new(t("Reinstall")),
                            )
                            .clicked()
                        {
                            action = Some(ModAction::Reinstall);
                        }
                        if let Some(number) = update_number {
                            if ui
                                .add_enabled(
                                    actions_enabled && target_ready,
                                    egui::Button::new(tf("Update to {}", &[number])),
                                )
                                .clicked()
                            {
                                action = Some(ModAction::Update);
                            }
                        }
                    } else if ui
                        .add_enabled(
                            actions_enabled && target_ready && filters_ready,
                            egui::Button::new(t("Install")),
                        )
                        .clicked()
                    {
                        action = Some(ModAction::Install);
                    }
                    ui.hyperlink_to(
                        t("Open on Modrinth"),
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
