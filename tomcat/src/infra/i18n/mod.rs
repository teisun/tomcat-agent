//! Product-owned text. Each process renders its own messages; keys never enter transcripts.
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub enum Locale {
    #[serde(rename = "en")]
    En,
    #[serde(rename = "zh-CN")]
    ZhCn,
}

impl Locale {
    pub fn from_language_tag(tag: &str) -> Self {
        let language = tag
            .trim()
            .split(['-', '_', '.', '@'])
            .next()
            .unwrap_or_default();
        if language.eq_ignore_ascii_case("zh") {
            Self::ZhCn
        } else {
            Self::En
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::En => "en",
            Self::ZhCn => "zh-CN",
        }
    }
}

// Entrypoints resolve the user's preference; uninitialized library callers use English.
static CURRENT: AtomicU8 = AtomicU8::new(0);

pub fn current_locale() -> Locale {
    if CURRENT.load(Ordering::Relaxed) == 1 {
        Locale::ZhCn
    } else {
        Locale::En
    }
}

pub fn set_locale(locale: Locale) {
    CURRENT.store(u8::from(locale == Locale::ZhCn), Ordering::Relaxed);
}

type Catalog = BTreeMap<String, String>;

fn catalogs() -> &'static (Catalog, Catalog) {
    static CATALOGS: OnceLock<(Catalog, Catalog)> = OnceLock::new();
    CATALOGS.get_or_init(|| {
        (
            serde_json::from_str(include_str!("../../../assets/i18n/en.json"))
                .expect("embedded English catalog"),
            serde_json::from_str(include_str!("../../../assets/i18n/zh-CN.json"))
                .expect("embedded Chinese catalog"),
        )
    })
}

fn lookup<'a>(locale: Locale, key: &str, en: &'a Catalog, zh: &'a Catalog) -> Option<&'a str> {
    let localized = if locale == Locale::ZhCn {
        zh.get(key)
    } else {
        None
    };
    localized.or_else(|| en.get(key)).map(String::as_str)
}

/// Single-pass interpolation: argument values are data, never another message template.
fn interpolate(template: &str, args: &[(&str, &str)]) -> Option<String> {
    let mut out = String::with_capacity(template.len());
    let mut remaining = template;
    while let Some(open) = remaining.find('{') {
        out.push_str(&remaining[..open]);
        let after = &remaining[open + 1..];
        let Some(close) = after.find('}') else {
            out.push_str(&remaining[open..]);
            return Some(out);
        };
        let name = &after[..close];
        let (_, value) = args.iter().find(|(key, _)| *key == name)?;
        out.push_str(value);
        remaining = &after[close + 1..];
    }
    out.push_str(remaining);
    Some(out)
}

pub fn tr_in(locale: Locale, key: &str, args: &[(&str, &str)]) -> String {
    let (en, zh) = catalogs();
    if let Some(message) =
        lookup(locale, key, en, zh).and_then(|template| interpolate(template, args))
    {
        return message;
    }
    // No argument values in diagnostics: they may contain user data.
    tracing::warn!(key, "missing product message or interpolation argument");
    lookup(locale, "error.unknown", en, zh)
        .unwrap_or("An unexpected error occurred.")
        .to_owned()
}

pub fn tr(key: &str, args: &[(&str, &str)]) -> String {
    tr_in(current_locale(), key, args)
}

#[cfg(test)]
mod tests;
