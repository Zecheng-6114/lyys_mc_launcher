//! Lightweight gettext-style translations embedded at compile time.
//!
//! Keys are the English source strings themselves, so code reads as
//! `t("Install instance")`. The locale files live in `locales/` and are
//! included into the binary, which keeps the launcher self-contained.

use std::collections::HashMap;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU8, Ordering};

/// Languages the launcher ships with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    EnUs,
    ZhCn,
    ZhTw,
    JaJp,
}

impl Language {
    pub const ALL: [Language; 4] = [Self::EnUs, Self::ZhCn, Self::ZhTw, Self::JaJp];

    /// Code persisted in the configuration and used by the locale file names.
    pub fn code(self) -> &'static str {
        match self {
            Self::EnUs => "en_us",
            Self::ZhCn => "zh_cn",
            Self::ZhTw => "zh_tw",
            Self::JaJp => "ja_jp",
        }
    }

    /// Native name shown in the language picker.
    pub fn label(self) -> &'static str {
        match self {
            Self::EnUs => "English",
            Self::ZhCn => "简体中文",
            Self::ZhTw => "繁體中文",
            Self::JaJp => "日本語",
        }
    }

    pub fn from_code(code: &str) -> Option<Self> {
        match code.trim().to_ascii_lowercase().as_str() {
            "en_us" => Some(Self::EnUs),
            "zh_cn" => Some(Self::ZhCn),
            "zh_tw" => Some(Self::ZhTw),
            "ja_jp" => Some(Self::JaJp),
            _ => None,
        }
    }

    /// Best effort guess from the environment; defaults to English.
    pub fn detect() -> Self {
        for key in ["LC_ALL", "LC_MESSAGES", "LANG"] {
            let Ok(value) = std::env::var(key) else {
                continue;
            };
            let value = value.to_ascii_lowercase();
            if value.starts_with("zh_cn") || value.starts_with("zh-hans") {
                return Self::ZhCn;
            }
            if value.starts_with("zh_tw")
                || value.starts_with("zh-hk")
                || value.starts_with("zh-hant")
            {
                return Self::ZhTw;
            }
            if value.starts_with("ja") {
                return Self::JaJp;
            }
            if value.starts_with("en") {
                return Self::EnUs;
            }
        }
        Self::EnUs
    }

    fn index(self) -> u8 {
        match self {
            Self::EnUs => 0,
            Self::ZhCn => 1,
            Self::ZhTw => 2,
            Self::JaJp => 3,
        }
    }
}

static CURRENT: AtomicU8 = AtomicU8::new(0);

pub fn set_language(language: Language) {
    CURRENT.store(language.index(), Ordering::Relaxed);
}

pub fn current() -> Language {
    match CURRENT.load(Ordering::Relaxed) {
        1 => Language::ZhCn,
        2 => Language::ZhTw,
        3 => Language::JaJp,
        _ => Language::EnUs,
    }
}

/// One parsed locale file.
struct Catalog {
    entries: HashMap<&'static str, &'static str>,
}

impl Catalog {
    fn parse(raw: &'static str) -> Self {
        let parsed: HashMap<String, String> = serde_json::from_str(raw).unwrap_or_default();
        let entries = parsed
            .into_iter()
            .map(|(key, value)| {
                let key: &'static str = Box::leak(key.into_boxed_str());
                let value: &'static str = Box::leak(value.into_boxed_str());
                (key, value)
            })
            .collect();
        Self { entries }
    }

    fn get(&self, key: &str) -> Option<&'static str> {
        self.entries.get(key).copied()
    }
}

struct Catalogs {
    en_us: Catalog,
    zh_cn: Catalog,
    zh_tw: Catalog,
    ja_jp: Catalog,
}

fn catalogs() -> &'static Catalogs {
    static CATALOGS: OnceLock<Catalogs> = OnceLock::new();
    CATALOGS.get_or_init(|| Catalogs {
        en_us: Catalog::parse(include_str!("../../locales/en_us.json")),
        zh_cn: Catalog::parse(include_str!("../../locales/zh_cn.json")),
        zh_tw: Catalog::parse(include_str!("../../locales/zh_tw.json")),
        ja_jp: Catalog::parse(include_str!("../../locales/ja_jp.json")),
    })
}

/// Translate a static key, falling back to English and then to the key itself.
pub fn t(key: &'static str) -> &'static str {
    let catalogs = catalogs();
    let catalog = match current() {
        Language::EnUs => &catalogs.en_us,
        Language::ZhCn => &catalogs.zh_cn,
        Language::ZhTw => &catalogs.zh_tw,
        Language::JaJp => &catalogs.ja_jp,
    };
    catalog
        .get(key)
        .or_else(|| catalogs.en_us.get(key))
        .unwrap_or(key)
}

/// Translate a key and replace its `{}` placeholders in order.
pub fn tf(key: &'static str, args: &[&str]) -> String {
    let template = t(key);
    if args.is_empty() {
        return template.to_owned();
    }
    let mut output =
        String::with_capacity(template.len() + args.iter().map(|arg| arg.len()).sum::<usize>());
    let mut rest = template;
    for arg in args {
        let Some(position) = rest.find("{}") else {
            break;
        };
        output.push_str(&rest[..position]);
        output.push_str(arg);
        rest = &rest[position + 2..];
    }
    output.push_str(rest);
    output
}
