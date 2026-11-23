use eframe::egui;

use crate::core::i18n::t;
use crate::gui::app::App;

impl App {
    pub(crate) fn modpack_form(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        ui.label(
            egui::RichText::new(t("modpack.file.hint"))
                .weak()
                .small(),
        );
        ui.horizontal(|ui| {
            ui.label(t("modpack.file.label"));
            ui.add(
                egui::TextEdit::singleline(&mut self.modpack_file)
                    .id_salt("modpack_file")
                    .desired_width(420.0),
            );
        });
        ui.checkbox(
            &mut self.modpack_with_optional,
            t("modpack.file.optional"),
        );
        ui.add_space(8.0);
        if ui
            .add_enabled(
                !self.modpack_installing,
                egui::Button::new(t("modpack.install")),
            )
            .clicked()
        {
            self.start_modpack_install(&ctx);
        }
        if self.modpack_installing {
            ui.horizontal(|ui| {
                ui.spinner();
                if let Some(progress) = &self.modpack_progress {
                    ui.label(progress);
                }
            });
        }
        if let Some(notice) = &self.modpack_notice {
            ui.label(egui::RichText::new(notice).color(egui::Color32::LIGHT_GREEN));
        }
        if let Some(error) = &self.modpack_error {
            ui.label(egui::RichText::new(error).color(ui.visuals().error_fg_color));
        }
    }
}
