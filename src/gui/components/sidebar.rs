use eframe::egui;

use crate::core::auth::SessionState;
use crate::core::i18n::{t, tf};
use crate::gui::app::{App, Page};

impl App {
    pub(crate) fn accounts_panel(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        egui::Panel::top("accounts").show(ui, |ui| {
            ui.add_space(6.0);
            ui.heading(t("Accounts"));
            for index in 0..self.accounts.len() {
                let uuid = self.accounts[index].profile.id.clone();
                let name = self.accounts[index].profile.name.clone();
                let state = self.accounts[index].state;
                let is_active = self.active.as_deref() == Some(uuid.as_str());
                ui.horizontal(|ui| {
                    if ui.selectable_label(is_active, name).clicked() {
                        match self.auth.set_active(&uuid) {
                            Ok(()) => self.active = Some(uuid.clone()),
                            Err(error) => {
                                let error = error.to_string();
                                self.log.push(tf(
                                    "failed to set active account: {}",
                                    &[error.as_str()],
                                ));
                            }
                        }
                    }
                    let state = match state {
                        SessionState::Valid => t("valid"),
                        SessionState::NeedsRelogin => t("needs relogin"),
                        SessionState::Offline => t("offline"),
                    };
                    ui.label(state);
                    if ui.small_button(t("Remove")).clicked() {
                        match self.auth.remove(&uuid) {
                            Ok(()) => self.reload_accounts(),
                            Err(error) => {
                                let error = error.to_string();
                                self.log
                                    .push(tf("failed to remove account: {}", &[error.as_str()]))
                            }
                        }
                    }
                });
            }
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label(t("Offline name:"));
                ui.text_edit_singleline(&mut self.offline_name);
                if ui
                    .add_enabled(!self.logging_in, egui::Button::new(t("Add offline")))
                    .clicked()
                {
                    let name = self.offline_name.trim().to_owned();
                    match self.auth.add_offline(&name) {
                        Ok(_) => {
                            self.offline_name.clear();
                            self.reload_accounts();
                        }
                        Err(error) => {
                            let error = error.to_string();
                            self.log
                                .push(tf("failed to add account: {}", &[error.as_str()]));
                        }
                    }
                }
                if ui
                    .add_enabled(
                        !self.logging_in,
                        egui::Button::new(t("Login with Microsoft")),
                    )
                    .clicked()
                {
                    self.start_login(&ctx);
                }
            });
            ui.add_space(6.0);
        });
    }

    pub(crate) fn page_tabs(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.selectable_value(&mut self.page, Page::Install, t("Install instance"));
            ui.selectable_value(&mut self.page, Page::Launch, t("Launch instance"));
            ui.selectable_value(&mut self.page, Page::Mods, t("Content"));
            ui.selectable_value(&mut self.page, Page::Settings, t("Settings"));
        });
    }
}
