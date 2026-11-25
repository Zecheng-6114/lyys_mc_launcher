mod error;
mod http;
mod minecraft;
mod model;
mod oauth;
mod offline;
mod store;
mod xbox;

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use store::{MicrosoftAccount, Store, StoredAccount};

pub use error::AuthError;
#[allow(unused_imports)]
pub use model::{AccountSession, DeviceCodePrompt, GameToken, Profile, SessionState};

const CLIENT_ID: &str = "f5caeb6d-1830-4c19-b809-83f41dcfd6bc";
const REFRESH_SKEW_SECONDS: u64 = 60;
const REQUEST_TIMEOUT_SECONDS: u64 = 30;

pub struct Auth {
    client: reqwest::Client,
    store: Store,
}

impl Auth {
    pub fn new() -> Result<Self, AuthError> {
        let client = reqwest::Client::builder()
            .user_agent(concat!("lyys_mc_launcher/", env!("CARGO_PKG_VERSION")))
            .timeout(Duration::from_secs(REQUEST_TIMEOUT_SECONDS))
            .build()?;
        Ok(Self {
            client,
            store: Store::new()?,
        })
    }

    pub async fn login<F>(&self, mut on_prompt: F) -> Result<AccountSession, AuthError>
    where
        F: FnMut(&DeviceCodePrompt),
    {
        let request = oauth::request_device_code(&self.client, CLIENT_ID).await?;
        on_prompt(&request.prompt);
        let tokens = oauth::poll_device_token(&self.client, CLIENT_ID, &request).await?;
        let xbox_user = xbox::user_token(&self.client, &tokens.access_token).await?;
        let xsts = xbox::xsts_token(&self.client, &xbox_user.token).await?;
        let credentials = minecraft::authenticate(&self.client, &xsts.token, &xsts.uhs).await?;

        let now = unix_now();
        let account = MicrosoftAccount {
            profile: credentials.profile,
            state: SessionState::Valid,
            ms_refresh_token: tokens.refresh_token,
            ms_access_token: tokens.access_token,
            ms_access_expires_at: now + tokens.expires_in,
            mc_access_token: credentials.access_token,
            mc_access_expires_at: now + credentials.expires_in,
        };
        self.store.save_microsoft(&account)?;
        self.ensure_active(&account.profile.id)?;
        Ok(AccountSession {
            profile: account.profile,
            state: account.state,
        })
    }

    pub fn add_offline(&self, name: &str) -> Result<AccountSession, AuthError> {
        let profile = offline::profile(name)?;
        self.store.save_offline(&profile)?;
        self.ensure_active(&profile.id)?;
        Ok(AccountSession {
            profile,
            state: SessionState::Offline,
        })
    }

    fn ensure_active(&self, uuid: &str) -> Result<(), AuthError> {
        if self.store.active()?.is_none() {
            self.store.set_active(uuid)?;
        }
        Ok(())
    }

    pub fn list(&self) -> Result<Vec<AccountSession>, AuthError> {
        let mut accounts: Vec<AccountSession> = self
            .store
            .list()?
            .into_iter()
            .map(|account| {
                let state = match &account {
                    StoredAccount::Microsoft(account) => account.state,
                    StoredAccount::Offline(_) => SessionState::Offline,
                };
                AccountSession {
                    profile: account.profile().clone(),
                    state,
                }
            })
            .collect();
        accounts.sort_by(|a, b| a.profile.name.cmp(&b.profile.name));
        Ok(accounts)
    }

    pub fn active_uuid(&self) -> Result<Option<String>, AuthError> {
        self.store.active()
    }

    pub fn set_active(&self, uuid: &str) -> Result<(), AuthError> {
        if self.store.find(uuid)?.is_none() {
            return Err(AuthError::UnknownAccount(uuid.to_owned()));
        }
        self.store.set_active(uuid)
    }

    pub fn remove(&self, uuid: &str) -> Result<(), AuthError> {
        if self.store.find(uuid)?.is_none() {
            return Err(AuthError::UnknownAccount(uuid.to_owned()));
        }
        self.store.remove(uuid)
    }

    pub async fn current_game_token(&self) -> Result<GameToken, AuthError> {
        let mut account = match self.load_active()? {
            StoredAccount::Microsoft(account) => account,
            StoredAccount::Offline(_) => return Err(AuthError::OfflineAccount),
        };
        if !self.is_fresh(&account) {
            self.refresh(&mut account).await?;
        }
        Ok(GameToken {
            access_token: account.mc_access_token,
            expires_at: account.mc_access_expires_at,
            profile: account.profile,
        })
    }

    pub async fn refresh_current(&self) -> Result<AccountSession, AuthError> {
        match self.load_active()? {
            StoredAccount::Offline(profile) => Ok(AccountSession {
                profile,
                state: SessionState::Offline,
            }),
            StoredAccount::Microsoft(mut account) => {
                if !self.is_fresh(&account) {
                    self.refresh(&mut account).await?;
                }
                Ok(AccountSession {
                    profile: account.profile,
                    state: account.state,
                })
            }
        }
    }

    fn load_active(&self) -> Result<StoredAccount, AuthError> {
        let uuid = self.store.active()?.ok_or(AuthError::NoActiveAccount)?;
        let account = self
            .store
            .find(&uuid)?
            .ok_or(AuthError::UnknownAccount(uuid))?;
        if let StoredAccount::Microsoft(microsoft) = &account {
            if microsoft.state == SessionState::NeedsRelogin {
                return Err(AuthError::NeedsRelogin);
            }
        }
        Ok(account)
    }

    fn is_fresh(&self, account: &MicrosoftAccount) -> bool {
        account.mc_access_expires_at > unix_now() + REFRESH_SKEW_SECONDS
    }

    async fn refresh(&self, account: &mut MicrosoftAccount) -> Result<(), AuthError> {
        let result = self.exchange(account).await;
        if matches!(result, Err(AuthError::NeedsRelogin)) {
            account.state = SessionState::NeedsRelogin;
            self.store.save_microsoft(account)?;
        }
        result
    }

    async fn exchange(&self, account: &mut MicrosoftAccount) -> Result<(), AuthError> {
        let tokens = oauth::refresh(&self.client, CLIENT_ID, &account.ms_refresh_token).await?;
        let xbox_user = xbox::user_token(&self.client, &tokens.access_token).await?;
        let xsts = xbox::xsts_token(&self.client, &xbox_user.token).await?;
        let credentials = minecraft::login(&self.client, &xsts.token, &xsts.uhs).await?;

        let now = unix_now();
        account.ms_access_token = tokens.access_token;
        account.ms_refresh_token = tokens.refresh_token;
        account.ms_access_expires_at = now + tokens.expires_in;
        account.mc_access_token = credentials.access_token;
        account.mc_access_expires_at = now + credentials.expires_in;
        account.state = SessionState::Valid;
        self.store.save_microsoft(account)
    }
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}
