use crate::infra::i18n::Locale;
use serde::{Deserialize, Serialize};

#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
pub enum UiLanguage {
    #[default]
    #[serde(rename = "auto")]
    Auto,
    #[serde(rename = "en")]
    En,
    #[serde(rename = "zh-CN")]
    ZhCn,
}

impl UiLanguage {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::En => "en",
            Self::ZhCn => "zh-CN",
        }
    }
    pub fn explicit_locale(self) -> Option<Locale> {
        match self {
            Self::Auto => None,
            Self::En => Some(Locale::En),
            Self::ZhCn => Some(Locale::ZhCn),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UiConfig {
    #[serde(default)]
    pub language: UiLanguage,
}
