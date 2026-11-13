use std::collections::HashMap;
use std::sync::OnceLock;

use gpui::SharedString;

pub const FALLBACK: Lang = Lang::EnUs;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lang {
    EnUs,
    ZhCn,
    JaJp,
}

impl Lang {
    pub fn from_tag(tag: &str) -> Option<Self> {
        match tag.trim().to_ascii_lowercase().as_str() {
            "en-us" | "en" => Some(Lang::EnUs),
            "zh-cn" | "zh-hans" | "zh" => Some(Lang::ZhCn),
            "ja-jp" | "ja" => Some(Lang::JaJp),
            _ => None,
        }
    }

    pub fn tag(self) -> &'static str {
        match self {
            Lang::EnUs => "en-US",
            Lang::ZhCn => "zh-CN",
            Lang::JaJp => "ja-JP",
        }
    }

    pub fn native_name(self) -> &'static str {
        match self {
            Lang::EnUs => "English",
            Lang::ZhCn => "简体中文",
            Lang::JaJp => "日本語",
        }
    }

    fn source(self) -> &'static str {
        match self {
            Lang::EnUs => include_str!("locales/en-US.json"),
            Lang::ZhCn => include_str!("locales/zh-CN.json"),
            Lang::JaJp => include_str!("locales/ja-JP.json"),
        }
    }

    fn table(self) -> &'static HashMap<String, String> {
        static EN_US: OnceLock<HashMap<String, String>> = OnceLock::new();
        static ZH_CN: OnceLock<HashMap<String, String>> = OnceLock::new();
        static JA_JP: OnceLock<HashMap<String, String>> = OnceLock::new();

        match self {
            Lang::EnUs => EN_US.get_or_init(|| parse(Lang::EnUs.source())),
            Lang::ZhCn => ZH_CN.get_or_init(|| parse(Lang::ZhCn.source())),
            Lang::JaJp => JA_JP.get_or_init(|| parse(Lang::JaJp.source())),
        }
    }
}

pub fn text(lang: Lang, key: &str) -> SharedString {
    match lookup(lang, key) {
        Some(value) => SharedString::from(value),
        None => SharedString::from(key.to_string()),
    }
}

pub fn text_args(lang: Lang, key: &str, args: &[(&str, &str)]) -> SharedString {
    let mut value = lookup(lang, key).unwrap_or_else(|| key.to_string());
    for (name, replacement) in args {
        value = value.replace(&format!("{{{name}}}"), replacement);
    }
    SharedString::from(value)
}

fn lookup(lang: Lang, key: &str) -> Option<String> {
    lang.table()
        .get(key)
        .or_else(|| FALLBACK.table().get(key))
        .cloned()
}

fn parse(source: &str) -> HashMap<String, String> {
    serde_json::from_str(source).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(lang: Lang) -> Vec<String> {
        let mut keys: Vec<String> = lang.table().keys().cloned().collect();
        keys.sort();
        keys
    }

    #[test]
    fn all_locales_share_the_same_keys() {
        let base = keys(FALLBACK);
        assert!(!base.is_empty());
        for lang in [Lang::ZhCn, Lang::JaJp] {
            assert_eq!(base, keys(lang), "locale {} key mismatch", lang.tag());
        }
    }

    #[test]
    fn missing_key_falls_back_to_key_name() {
        assert_eq!(text(FALLBACK, "no.such.key").as_ref(), "no.such.key");
        assert_eq!(text(Lang::ZhCn, "no.such.key").as_ref(), "no.such.key");
    }

    #[test]
    fn placeholders_are_replaced() {
        let value = text_args(Lang::ZhCn, "task.title.launch", &[("name", "Demo")]);
        assert_eq!(value.as_ref(), "启动 Demo");
    }

    #[test]
    fn unknown_language_tag_is_rejected() {
        assert_eq!(Lang::from_tag("fr-FR"), None);
        assert_eq!(Lang::from_tag("zh-CN"), Some(Lang::ZhCn));
    }
}
