use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::Write;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use thiserror::Error;

const CONFIG_FILE: &str = "config.json";
const DATA_DIR_ENV: &str = "LYYS_DATA_DIR";
const DATA_DIR_NAME: &str = ".lyysml";

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("failed to read or write the configuration: {0}")]
    Io(#[from] std::io::Error),
    #[error("failed to parse the configuration: {0}")]
    Parse(#[from] serde_json::Error),
}

/// Configuration of a single instance that overrides the global one.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstanceConfig {
    /// True means the global configuration is used and the fields below are ignored.
    #[serde(default = "default_true")]
    pub use_global: bool,
    #[serde(default)]
    pub isolate: bool,
    /// Empty means the global Java path is used.
    #[serde(default)]
    pub java_path: String,
}

impl Default for InstanceConfig {
    fn default() -> Self {
        Self {
            use_global: true,
            isolate: false,
            java_path: String::new(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LauncherConfig {
    /// Root of the Minecraft installation; empty means `<cwd>/.minecraft`.
    #[serde(default)]
    pub minecraft_dir: String,
    /// Java executable; empty means auto-detect.
    #[serde(default)]
    pub java_path: String,
    #[serde(default)]
    pub isolate: bool,
    /// Language code (`en_us`, `zh_cn`, `zh_tw`, `ja_jp`); empty means auto-detect.
    #[serde(default)]
    pub language: String,
    #[serde(default)]
    pub instances: BTreeMap<String, InstanceConfig>,
}

impl LauncherConfig {
    pub fn load() -> Result<Self, ConfigError> {
        let path = path()?;
        if !path.is_file() {
            return Ok(Self::default());
        }
        Ok(serde_json::from_slice(&fs::read(path)?)?)
    }

    pub fn save(&self) -> Result<PathBuf, ConfigError> {
        let path = path()?;
        let parent = path.parent().expect("config path has a parent");
        fs::create_dir_all(parent)?;
        let bytes = serde_json::to_vec_pretty(self)?;
        let temp = parent.join(format!(".{CONFIG_FILE}.{}.tmp", std::process::id()));
        let written = (|| -> Result<(), ConfigError> {
            let mut file = File::create(&temp)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
            drop(file);
            fs::rename(&temp, &path)?;
            Ok(())
        })();
        if written.is_err() {
            let _ = fs::remove_file(&temp);
        }
        written?;
        Ok(path)
    }

    /// Version isolation that applies to `instance`.
    pub fn effective_isolate(&self, instance: &str) -> bool {
        match self.instances.get(instance) {
            Some(config) if !config.use_global => config.isolate,
            _ => self.isolate,
        }
    }

    /// Java path that applies to `instance`; empty means auto-detect.
    pub fn effective_java(&self, instance: &str) -> &str {
        match self.instances.get(instance) {
            Some(config) if !config.use_global && !config.java_path.trim().is_empty() => {
                config.java_path.trim()
            }
            _ => self.java_path.trim(),
        }
    }
}

/// Location of the configuration file.
pub fn path() -> Result<PathBuf, ConfigError> {
    Ok(data_root()?.join(CONFIG_FILE))
}

fn default_true() -> bool {
    true
}

fn data_root() -> Result<PathBuf, ConfigError> {
    // 与账号数据使用同一个根目录，便于整体备份或迁移
    if let Some(dir) = std::env::var_os(DATA_DIR_ENV).filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(dir));
    }
    Ok(std::env::current_dir()?.join(DATA_DIR_NAME))
}
