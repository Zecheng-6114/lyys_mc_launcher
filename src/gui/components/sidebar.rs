use eframe::egui;

use crate::core::auth::SessionState;
use crate::core::i18n::{t, tf};
use crate::gui::app::{App, Page};

impl App {
    pub(crate) fn accounts_panel(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        egui::Panel::top("accounts").show(ui, |ui| {
            ui.add_space(6.0);
            ui.heading(t("accounts.title"));
            let accounts = self.accounts.clone();
            for account in &accounts {
                let uuid = account.profile.id.clone();
                let name = account.profile.name.clone();
                let state = account.state;
                let is_active = self.active.as_deref() == Some(uuid.as_str());
                ui.horizontal(|ui| {
                    if ui.selectable_label(is_active, name).clicked() {
                        match self.auth.set_active(&uuid) {
                            Ok(()) => self.active = Some(uuid.clone()),
                            Err(error) => {
                                let detail = error.message();
                                self.log.push(tf(
                                    "error.account_active",
                                    &[("error", detail.as_str())],
                                ));
                            }
                        }
                    }
                    let state = match state {
                        SessionState::Valid => t("account.state.valid"),
                        SessionState::NeedsRelogin => t("account.state.relogin"),
                        SessionState::Offline => t("account.state.offline"),
                    };
                    ui.label(state);
                    if ui.small_button(t("common.remove")).clicked() {
                        match self.auth.remove(&uuid) {
                            Ok(()) => self.reload_accounts(),
                            Err(error) => {
                                let detail = error.message();
                                self.log.push(tf(
                                    "error.account_remove",
                                    &[("error", detail.as_str())],
                                ))
                            }
                        }
                    }
                });
            }
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label(t("accounts.offline_name"));
                ui.text_edit_singleline(&mut self.offline_name);
                if ui
                    .add_enabled(!self.logging_in, egui::Button::new(t("accounts.add_offline")))
                    .clicked()
                {
                    let name = self.offline_name.trim().to_owned();
                    match self.auth.add_offline(&name) {
                        Ok(_) => {
                            self.offline_name.clear();
                            self.reload_accounts();
                        }
                        Err(error) => {
                            let detail = error.message();
                            self.log
                                .push(tf("error.account_add", &[("error", detail.as_str())]));
                        }
                    }
                }
                if ui
                    .add_enabled(
                        !self.logging_in,
                        egui::Button::new(t("accounts.login_microsoft")),
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
            ui.selectable_value(&mut self.page, Page::Launch, t("nav.launch"));
            ui.selectable_value(&mut self.page, Page::Install, t("nav.content"));
            ui.selectable_value(&mut self.page, Page::Settings, t("nav.settings"));
        });
    }
}
