use thiserror::Error;

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
