use thiserror::Error;

use crate::core::i18n::{t, tf};

#[derive(Debug, Error)]
pub enum AuthError {
    #[error("network request failed: {0}")]
    Network(#[from] reqwest::Error),
    #[error("cannot access login data: {0}")]
    Io(#[from] std::io::Error),
    #[error("login data is corrupted: {0}")]
    Data(#[from] serde_json::Error),
    #[error("authentication service returned an unexpected response: {0}")]
    Protocol(String),
    #[error("the device code has expired, please log in again")]
    DeviceCodeExpired,
    #[error("the login session has expired, please log in again for this account")]
    NeedsRelogin,
    #[error("{0}")]
    Xbox(String),
    #[error("this Microsoft account does not own Minecraft: Java Edition")]
    NoOwnership,
    #[error("this Microsoft account has no Minecraft profile yet")]
    NoProfile,
    #[error("no account is selected")]
    NoActiveAccount,
    #[error("account {0} does not exist")]
    UnknownAccount(String),
    #[error("invalid account id: {0}")]
    InvalidUuid(String),
    #[error("invalid offline username: {0}")]
    InvalidName(String),
    #[error("offline accounts have no game token")]
    OfflineAccount,
}

impl AuthError {
    /// Localized text for the interface. File logs use `Display`, which keeps
    /// the English diagnostic regardless of the selected language.
    pub fn message(&self) -> String {
        match self {
            Self::DeviceCodeExpired => t("cli.error.auth_expired_code").to_owned(),
            Self::NeedsRelogin => t("cli.error.auth_relogin").to_owned(),
            Self::NoOwnership => t("cli.error.auth_no_ownership").to_owned(),
            Self::NoProfile => t("cli.error.auth_no_profile").to_owned(),
            Self::NoActiveAccount => t("cli.error.auth_no_active").to_owned(),
            Self::OfflineAccount => t("cli.error.auth_offline_token").to_owned(),
            Self::UnknownAccount(account) => {
                tf("cli.error.auth_unknown_account", &[("account", account.as_str())])
            }
            Self::InvalidUuid(id) => tf("cli.error.auth_invalid_uuid", &[("id", id.as_str())]),
            Self::InvalidName(name) => {
                tf("cli.error.auth_invalid_name", &[("name", name.as_str())])
            }
            other => {
                let detail = other.to_string();
                tf("cli.error.auth", &[("error", detail.as_str())])
            }
        }
    }
}
