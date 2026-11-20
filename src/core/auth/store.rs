use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::error::AuthError;
use super::model::{Profile, SessionState};

const ACTIVE_FILE: &str = "active.json";
const MICROSOFT_DIR: &str = "microsoft";
const OFFLINE_DIR: &str = "offline";
const DATA_DIR_ENV: &str = "LYYS_DATA_DIR";
const DATA_DIR_NAME: &str = ".lyysml";

#[derive(Serialize, Deserialize)]
pub struct MicrosoftAccount {
    pub profile: Profile,
    pub state: SessionState,
    pub ms_refresh_token: String,
    pub ms_access_token: String,
    pub ms_access_expires_at: u64,
    pub mc_access_token: String,
    pub mc_access_expires_at: u64,
}

pub enum StoredAccount {
    Microsoft(MicrosoftAccount),
    Offline(Profile),
}

impl StoredAccount {
    pub fn profile(&self) -> &Profile {
        match self {
            Self::Microsoft(account) => &account.profile,
            Self::Offline(profile) => profile,
        }
    }
}

#[derive(Serialize, Deserialize)]
struct ActiveAccount {
    uuid: String,
}

pub struct Store {
    root: PathBuf,
}

impl Store {
    pub fn new() -> Result<Self, AuthError> {
        Ok(Self {
            root: data_root()?.join("accounts"),
        })
    }

    pub fn save_microsoft(&self, account: &MicrosoftAccount) -> Result<(), AuthError> {
        write_atomic(
            &self.microsoft_path(&account.profile.id)?,
            &serde_json::to_vec_pretty(account)?,
        )
    }

    pub fn save_offline(&self, profile: &Profile) -> Result<(), AuthError> {
        write_atomic(
            &self.offline_path(&profile.id)?,
            &serde_json::to_vec_pretty(profile)?,
        )
    }

    pub fn list(&self) -> Result<Vec<StoredAccount>, AuthError> {
        let mut accounts = Vec::new();
        let microsoft_dir = self.root.join(MICROSOFT_DIR);
        if microsoft_dir.exists() {
            for entry in fs::read_dir(microsoft_dir)? {
                let path = entry?.path();
                if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
                    continue;
                }
                // 单个文件损坏时跳过，避免一个坏文件让整个账号列表不可用
                if let Ok(bytes) = fs::read(&path) {
                    if let Ok(account) = serde_json::from_slice::<MicrosoftAccount>(&bytes) {
                        accounts.push(StoredAccount::Microsoft(account));
                    }
                }
            }
        }
        let offline_dir = self.root.join(OFFLINE_DIR);
        if offline_dir.exists() {
            for entry in fs::read_dir(offline_dir)? {
                let path = entry?.path();
                if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
                    continue;
                }
                if let Ok(bytes) = fs::read(&path) {
                    if let Ok(profile) = serde_json::from_slice::<Profile>(&bytes) {
                        accounts.push(StoredAccount::Offline(profile));
                    }
                }
            }
        }
        Ok(accounts)
    }

    pub fn find(&self, uuid: &str) -> Result<Option<StoredAccount>, AuthError> {
        let offline = self.offline_path(uuid)?;
        if offline.exists() {
            return Ok(Some(StoredAccount::Offline(serde_json::from_slice(
                &fs::read(offline)?,
            )?)));
        }
        let path = self.microsoft_path(uuid)?;
        if !path.exists() {
            return Ok(None);
        }
        Ok(Some(StoredAccount::Microsoft(serde_json::from_slice(
            &fs::read(path)?,
        )?)))
    }

    pub fn remove(&self, uuid: &str) -> Result<(), AuthError> {
        let offline = self.offline_path(uuid)?;
        if offline.exists() {
            fs::remove_file(offline)?;
        }
        let path = self.microsoft_path(uuid)?;
        if path.exists() {
            fs::remove_file(path)?;
        }
        if self.active()?.as_deref() == Some(uuid) {
            let active = self.root.join(ACTIVE_FILE);
            if active.exists() {
                fs::remove_file(active)?;
            }
        }
        Ok(())
    }

    pub fn active(&self) -> Result<Option<String>, AuthError> {
        let path = self.root.join(ACTIVE_FILE);
        if !path.exists() {
            return Ok(None);
        }
        let active: ActiveAccount = serde_json::from_slice(&fs::read(path)?)?;
        Ok(Some(active.uuid))
    }

    pub fn set_active(&self, uuid: &str) -> Result<(), AuthError> {
        let active = ActiveAccount {
            uuid: uuid.to_owned(),
        };
        write_atomic(&self.root.join(ACTIVE_FILE), &serde_json::to_vec(&active)?)
    }

    fn microsoft_path(&self, uuid: &str) -> Result<PathBuf, AuthError> {
        Ok(self
            .root
            .join(MICROSOFT_DIR)
            .join(format!("{}.json", checked_uuid(uuid)?)))
    }

    fn offline_path(&self, uuid: &str) -> Result<PathBuf, AuthError> {
        Ok(self
            .root
            .join(OFFLINE_DIR)
            .join(format!("{}.json", checked_uuid(uuid)?)))
    }
}

// 直接覆盖写入在中断时会留下半个文件，先写同目录临时文件再改名可以保住旧内容
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), AuthError> {
    let parent = path.parent().expect("account path has a parent");
    fs::create_dir_all(parent)?;
    let temp = parent.join(format!(
        ".{}.{}.tmp",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("account"),
        std::process::id()
    ));
    let written = (|| -> Result<(), AuthError> {
        let mut file = File::create(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temp, path)?;
        Ok(())
    })();
    if written.is_err() {
        let _ = fs::remove_file(&temp);
    }
    written
}

fn checked_uuid(uuid: &str) -> Result<&str, AuthError> {
    // UUID 直接参与文件名，必须拦截非法输入以避免路径穿越
    if uuid.len() != 32 || !uuid.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(AuthError::InvalidUuid(uuid.to_owned()));
    }
    Ok(uuid)
}

fn data_root() -> Result<PathBuf, AuthError> {
    // 环境变量优先，便于测试与多实例隔离；空值视为未设置
    if let Some(dir) = std::env::var_os(DATA_DIR_ENV).filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(dir));
    }
    // 数据跟着启动时的工作目录走，从哪个目录启动就落在哪个目录
    Ok(std::env::current_dir()?.join(DATA_DIR_NAME))
}
