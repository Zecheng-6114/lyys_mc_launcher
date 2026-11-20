use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Profile {
    pub id: String,
    pub name: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionState {
    Valid,
    NeedsRelogin,
    Offline,
}

#[derive(Clone, Debug)]
pub struct AccountSession {
    pub profile: Profile,
    pub state: SessionState,
}

#[allow(dead_code)]
#[derive(Clone, Debug)]
pub struct GameToken {
    pub access_token: String,
    pub expires_at: u64,
    pub profile: Profile,
}

#[derive(Clone, Debug)]
pub struct DeviceCodePrompt {
    pub verification_uri: String,
    pub user_code: String,
    pub expires_in: u64,
}
