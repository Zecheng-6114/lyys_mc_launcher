use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

const CONFIG_FILE: &str = "config.json";
const DATA_DIR_ENV: &str = "LYYS_DATA_DIR";
const DATA_DIR_NAME: &str = ".lyysml";
/// 没有配置任何目录时使用的默认 Minecraft 目录（相对于启动器工作目录）。
pub const DEFAULT_MINECRAFT_DIR: &str = ".minecraft";
/// 内存分配上限（MiB），GUI 与 CLI 共用。
pub const MAX_MEMORY_MB: u32 = 65536;

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
    /// Maximum Java heap in MiB; zero means the global value is used.
    #[serde(default)]
    pub memory_mb: u32,
}

impl Default for InstanceConfig {
    fn default() -> Self {
        Self {
            use_global: true,
            isolate: false,
            java_path: String::new(),
            memory_mb: 0,
        }
    }
}

/// 一个 Minecraft 目录及其下的实例覆盖配置。
///
/// 实例以版本 id 为键，但只在所属目录内生效，不同目录下的同名版本互不影响。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MinecraftDirConfig {
    /// 显示名称；为空时直接显示路径。
    #[serde(default)]
    pub name: String,
    /// Minecraft 目录路径；空路径是无效条目，保存时会被丢弃。
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub instances: BTreeMap<String, InstanceConfig>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LauncherConfig {
    /// 由启动器管理的 Minecraft 目录。
    #[serde(default)]
    pub minecraft_dirs: Vec<MinecraftDirConfig>,
    /// 当前使用的目录路径；为空时使用第一个目录。
    #[serde(default)]
    pub active_minecraft_dir: String,
    /// Java executable; empty means auto-detect.
    #[serde(default)]
    pub java_path: String,
    /// Java executables remembered from earlier scans, used as extra candidates.
    #[serde(default)]
    pub java_versions: Vec<String>,
    #[serde(default)]
    pub isolate: bool,
    /// Maximum Java heap in MiB; zero leaves the JVM default.
    #[serde(default)]
    pub memory_mb: u32,
    /// Language code (`en_us`, `zh_cn`, `zh_tw`, `ja_jp`); empty means auto-detect.
    #[serde(default)]
    pub language: String,
    /// Download mirror root; empty means the official sources.
    #[serde(default)]
    pub download_mirror: String,
    /// 旧版配置的单一目录，用于加载时迁移；保存时同步为当前目录，供旧版本读取。
    #[serde(default, rename = "minecraft_dir")]
    legacy_minecraft_dir: String,
    /// 旧版配置的扁平实例覆盖表，用于加载时迁移；保存时同步为当前目录的实例。
    #[serde(default, rename = "instances")]
    legacy_instances: BTreeMap<String, InstanceConfig>,
}

impl LauncherConfig {
    pub fn load() -> Result<Self, ConfigError> {
        let path = path()?;
        if !path.is_file() {
            return Ok(Self::default());
        }
        let mut config: Self = serde_json::from_slice(&fs::read(path)?)?;
        config.migrate();
        Ok(config)
    }

    /// 把旧版的单一目录和扁平实例表迁移成按目录分组的结构。
    fn migrate(&mut self) {
        let legacy_path = self.legacy_minecraft_dir.trim().to_owned();
        let legacy_instances = std::mem::take(&mut self.legacy_instances);
        if self.minecraft_dirs.is_empty() {
            if !legacy_path.is_empty() || !legacy_instances.is_empty() {
                self.minecraft_dirs.push(MinecraftDirConfig {
                    name: String::new(),
                    path: if legacy_path.is_empty() {
                        DEFAULT_MINECRAFT_DIR.to_owned()
                    } else {
                        legacy_path
                    },
                    instances: legacy_instances,
                });
            }
        } else if !legacy_instances.is_empty() {
            // 新结构已经存在时不能直接丢弃旧覆盖，把它们补进对应的目录
            let target = if legacy_path.is_empty() {
                self.active_index().unwrap_or(0)
            } else {
                self.dir_index(&legacy_path).unwrap_or(0)
            };
            let instances = &mut self.minecraft_dirs[target].instances;
            for (id, entry) in legacy_instances {
                // 新结构里的条目优先，旧数据只补空缺
                instances.entry(id).or_insert(entry);
            }
        }
        self.legacy_minecraft_dir.clear();
        self.normalize();
    }

    /// 清理目录列表：丢弃空路径、合并重复目录、保证至少有一个目录，
    /// 并让 `active_minecraft_dir` 指向一个真实存在的条目。
    pub fn normalize(&mut self) {
        let mut dirs: Vec<MinecraftDirConfig> = Vec::with_capacity(self.minecraft_dirs.len());
        for mut dir in std::mem::take(&mut self.minecraft_dirs) {
            dir.path = dir.path.trim().to_owned();
            dir.name = dir.name.trim().to_owned();
            if dir.path.is_empty() {
                continue;
            }
            if let Some(existing) = dirs.iter_mut().find(|kept| same_dir(&kept.path, &dir.path)) {
                // 同一目录登记了多次时保留先出现的那条，把后一条的实例补进来
                for (id, entry) in dir.instances {
                    existing.instances.entry(id).or_insert(entry);
                }
                continue;
            }
            dirs.push(dir);
        }
        if dirs.is_empty() {
            dirs.push(MinecraftDirConfig {
                path: DEFAULT_MINECRAFT_DIR.to_owned(),
                ..Default::default()
            });
        }
        self.minecraft_dirs = dirs;
        let active = self.active_minecraft_dir.trim().to_owned();
        self.active_minecraft_dir = self
            .minecraft_dirs
            .iter()
            .find(|dir| same_dir(&dir.path, &active))
            .or_else(|| self.minecraft_dirs.first())
            .map(|dir| dir.path.clone())
            .unwrap_or_default();
    }

    pub fn save(&mut self) -> Result<PathBuf, ConfigError> {
        self.normalize();
        // 把当前目录同步到旧版字段，旧版本启动器读到的仍是有效配置；
        // 新版本读取时以 minecraft_dirs 为准，不会被这两份副本覆盖。
        let (legacy_path, legacy_instances) = self
            .active_dir()
            .map(|dir| (dir.path.clone(), dir.instances.clone()))
            .unwrap_or_default();
        self.legacy_minecraft_dir = legacy_path;
        self.legacy_instances = legacy_instances;
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

    /// 当前目录在列表中的位置；列表为空时返回 `None`。
    ///
    /// 路径找不到对应条目时退回第一个目录，保证“当前目录”只有一个定义。
    pub fn active_index(&self) -> Option<usize> {
        if self.minecraft_dirs.is_empty() {
            return None;
        }
        let active = self.active_minecraft_dir.trim();
        if !active.is_empty() {
            if let Some(index) = self.dir_index(active) {
                return Some(index);
            }
        }
        Some(0)
    }

    /// 已登记目录中与 `path` 相同的位置；不会新增条目。
    pub fn dir_index(&self, path: &str) -> Option<usize> {
        let path = path.trim();
        if path.is_empty() {
            return None;
        }
        // 先比字符串，命中时省掉一次 `absolute` 的文件系统调用
        self.minecraft_dirs
            .iter()
            .position(|dir| dir.path.trim() == path || same_dir(&dir.path, path))
    }

    /// 当前使用的目录。
    pub fn active_dir(&self) -> Option<&MinecraftDirConfig> {
        self.active_index()
            .and_then(|index| self.minecraft_dirs.get(index))
    }

    pub fn active_dir_mut(&mut self) -> Option<&mut MinecraftDirConfig> {
        let index = self.active_index()?;
        self.minecraft_dirs.get_mut(index)
    }

    /// 添加 `path`（若尚不存在）并设为当前目录。
    pub fn select_dir(&mut self, path: &str) {
        let path = path.trim();
        if path.is_empty() {
            return;
        }
        let index = match self.dir_index(path) {
            Some(index) => index,
            None => {
                self.minecraft_dirs.push(MinecraftDirConfig {
                    name: String::new(),
                    path: path.to_owned(),
                    ..Default::default()
                });
                self.minecraft_dirs.len() - 1
            }
        };
        self.active_minecraft_dir = self.minecraft_dirs[index].path.trim().to_owned();
    }

    /// 登记 `path`（若尚未登记），不改变当前目录。
    pub fn register_dir(&mut self, path: &str) {
        let path = path.trim();
        if path.is_empty() || self.dir_index(path).is_some() {
            return;
        }
        self.minecraft_dirs.push(MinecraftDirConfig {
            name: String::new(),
            path: path.to_owned(),
            ..Default::default()
        });
    }

    /// `path` 目录下的实例覆盖表；目录未登记时返回 `None`，不会隐式新增目录。
    pub fn dir_instances_mut(
        &mut self,
        path: &Path,
    ) -> Option<&mut BTreeMap<String, InstanceConfig>> {
        let raw = path.to_string_lossy();
        let index = self.dir_index(raw.trim())?;
        Some(&mut self.minecraft_dirs[index].instances)
    }

    /// Version isolation that applies to `instance` in the active directory.
    pub fn effective_isolate(&self, instance: &str) -> bool {
        match self
            .active_dir()
            .and_then(|dir| dir.instances.get(instance))
        {
            Some(config) if !config.use_global => config.isolate,
            _ => self.isolate,
        }
    }

    /// Java path that applies to `instance` in the active directory; empty means auto-detect.
    pub fn effective_java(&self, instance: &str) -> &str {
        let configured = self
            .active_dir()
            .and_then(|dir| dir.instances.get(instance))
            .filter(|config| !config.use_global)
            .map(|config| config.java_path.trim())
            .unwrap_or("");
        if configured.is_empty() {
            self.java_path.trim()
        } else {
            configured
        }
    }

    /// Maximum Java heap in MiB that applies to `instance` in the active
    /// directory; zero means the JVM default.
    pub fn effective_memory(&self, instance: &str) -> u32 {
        let configured = self
            .active_dir()
            .and_then(|dir| dir.instances.get(instance))
            .filter(|config| !config.use_global)
            .map(|config| config.memory_mb)
            .unwrap_or(0);
        if configured == 0 {
            self.memory_mb
        } else {
            configured
        }
    }
}

/// 两个路径是否指向同一个目录：先统一成绝对路径，Windows 上忽略大小写。
///
/// 不解析符号链接；无法绝对化时退回原始字符串比较。
fn same_dir(left: &str, right: &str) -> bool {
    let left = left.trim();
    let right = right.trim();
    if left.is_empty() || right.is_empty() {
        return false;
    }
    match (std::path::absolute(left), std::path::absolute(right)) {
        (Ok(left), Ok(right)) => paths_equal(&left, &right),
        _ => paths_equal(Path::new(left), Path::new(right)),
    }
}

fn paths_equal(left: &Path, right: &Path) -> bool {
    #[cfg(windows)]
    {
        // Windows 文件系统不区分大小写，`C:\MC` 与 `c:\mc` 是同一个目录
        let left = left.as_os_str().to_string_lossy();
        let right = right.as_os_str().to_string_lossy();
        left.eq_ignore_ascii_case(&right)
    }
    #[cfg(not(windows))]
    {
        left == right
    }
}

/// Location of the configuration file.
pub fn path() -> Result<PathBuf, ConfigError> {
    Ok(data_root()?.join(CONFIG_FILE))
}

/// Directory holding cached remote data such as version lists.
pub fn cache_dir() -> Result<PathBuf, ConfigError> {
    Ok(data_root()?.join("cache"))
}

/// Directory for logs produced by external tools such as loader installers.
pub fn log_dir() -> Result<PathBuf, ConfigError> {
    Ok(data_root()?.join("log"))
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
