use std::sync::Arc;

use eframe::egui;

use crate::core::auth::SessionState;
use crate::core::i18n::{t, tf};
use crate::core::logging;
use crate::core::region::Region;
use crate::gui::app::{App, Message, Page};

impl App {
    pub(crate) fn accounts_panel(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        egui::Panel::top("accounts").show(ui, |ui| {
            ui.add_space(6.0);
            ui.heading(t("accounts.title"));
            let accounts = self.accounts.clone();
            let has_valid_microsoft = accounts
                .iter()
                .any(|account| account.state == SessionState::Valid);
            let offline_blocked =
                self.region == Some(Region::Other) && !has_valid_microsoft;
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
                                logging::error("auth", &error.to_string());
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
                                logging::error("auth", &error.to_string());
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
                let enabled =
                    !self.logging_in && !self.offline_adding && !offline_blocked;
                let response = ui.add_enabled(
                    enabled,
                    egui::Button::new(t("accounts.add_offline")),
                );
                let response = if offline_blocked {
                    response.on_disabled_hover_text(t("accounts.offline_requires_login"))
                } else {
                    response
                };
                if response.clicked() {
                    let name = self.offline_name.trim().to_owned();
                    self.offline_adding = true;
                    let auth = Arc::clone(&self.auth);
                    let tx = self.tx.clone();
                    let ctx = ui.ctx().clone();
                    self.handle.spawn(async move {
                        let result = auth.add_offline(&name).await.map(|_| ());
                        let _ = tx.send(Message::OfflineAdded { name, result });
                        ctx.request_repaint();
                    });
                }
                if ui
                    .add_enabled(
                        !self.logging_in && !self.offline_adding,
                        egui::Button::new(t("accounts.login_microsoft")),
                    )
                    .clicked()
                {
                    self.start_login(&ctx);
                }
                if self.offline_adding {
                    ui.spinner();
                }
            });
            if offline_blocked {
                ui.label(
                    egui::RichText::new(t("accounts.offline_requires_login"))
                        .weak()
                        .small(),
                );
            }
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
