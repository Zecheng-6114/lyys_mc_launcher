//! Lightweight gettext-style translations embedded at compile time.
//!
//! Keys are semantic identifiers such as `nav.install` or `error.save_failed`.
//! Placeholders in the values are named, e.g. `"downloaded {downloaded} files"`,
//! and are substituted by [`tf`]. The locale files live in `locales/` and are
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

    /// Parse a configuration code or a locale tag such as `zh-CN` / `ja_JP`.
    pub fn from_code(code: &str) -> Option<Self> {
        let tag = code.trim().to_ascii_lowercase().replace('-', "_");
        let primary = tag.split('.').next().unwrap_or("");
        if primary.starts_with("zh") {
            let traditional = ["tw", "hk", "mo", "hant"]
                .iter()
                .any(|part| primary.contains(part));
            return Some(if traditional { Self::ZhTw } else { Self::ZhCn });
        }
        if primary.starts_with("ja") {
            return Some(Self::JaJp);
        }
        if primary.starts_with("en") {
            return Some(Self::EnUs);
        }
        None
    }

    /// Best effort guess from the environment; defaults to English.
    pub fn detect() -> Self {
        for key in ["LC_ALL", "LC_MESSAGES", "LANG"] {
            let Ok(value) = std::env::var(key) else {
                continue;
            };
            let value = value.trim().to_ascii_lowercase();
            if value.is_empty() || value == "c" || value == "posix" {
                continue;
            }
            if let Some(language) = Self::from_code(&value) {
                return language;
            }
        }
        Self::EnUs
    }

    fn source(self) -> &'static str {
        match self {
            Self::EnUs => include_str!("../../locales/en_us.json"),
            Self::ZhCn => include_str!("../../locales/zh_cn.json"),
            Self::ZhTw => include_str!("../../locales/zh_tw.json"),
            Self::JaJp => include_str!("../../locales/ja_jp.json"),
        }
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

fn parse(source: &str) -> HashMap<String, String> {
    serde_json::from_str(source).unwrap_or_default()
}

/// Parsed locale file; initialized once and kept for the whole process.
fn catalog(language: Language) -> &'static HashMap<String, String> {
    static EN_US: OnceLock<HashMap<String, String>> = OnceLock::new();
    static ZH_CN: OnceLock<HashMap<String, String>> = OnceLock::new();
    static ZH_TW: OnceLock<HashMap<String, String>> = OnceLock::new();
    static JA_JP: OnceLock<HashMap<String, String>> = OnceLock::new();

    match language {
        Language::EnUs => EN_US.get_or_init(|| parse(Language::EnUs.source())),
        Language::ZhCn => ZH_CN.get_or_init(|| parse(Language::ZhCn.source())),
        Language::ZhTw => ZH_TW.get_or_init(|| parse(Language::ZhTw.source())),
        Language::JaJp => JA_JP.get_or_init(|| parse(Language::JaJp.source())),
    }
}

/// Translate a static key, falling back to English and then to the key itself.
pub fn t(key: &'static str) -> &'static str {
    if let Some(value) = catalog(current()).get(key) {
        return value;
    }
    catalog(Language::EnUs)
        .get(key)
        .map(String::as_str)
        .unwrap_or(key)
}

/// Translate a key and substitute `{name}` placeholders from `args`.
pub fn tf(key: &'static str, args: &[(&str, &str)]) -> String {
    let template = t(key);
    if args.is_empty() || !template.contains('{') {
        return template.to_owned();
    }
    let mut output = String::with_capacity(
        template.len() + args.iter().map(|(_, value)| value.len()).sum::<usize>(),
    );
    let mut rest = template;
    while let Some(start) = rest.find('{') {
        output.push_str(&rest[..start]);
        let tail = &rest[start + 1..];
        let Some(end) = tail.find('}') else {
            output.push_str(&rest[start..]);
            return output;
        };
        let name = &tail[..end];
        match args.iter().find(|(arg, _)| *arg == name) {
            Some((_, value)) => output.push_str(value),
            None => {
                output.push('{');
                output.push_str(name);
                output.push('}');
            }
        }
        rest = &tail[end + 1..];
    }
    output.push_str(rest);
    output
}
