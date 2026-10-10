use super::super::*;
use std::collections::BTreeSet;

fn parameters(text: &str) -> BTreeSet<String> {
    let re = regex::Regex::new(r"\{([A-Za-z_][A-Za-z0-9_]*)\}").unwrap();
    re.captures_iter(text).map(|c| c[1].to_owned()).collect()
}

#[test]
fn i18n_catalog_parity() {
    let (en, zh) = catalogs();
    for (key, text) in en {
        assert!(!text.trim().is_empty(), "empty English value {key}");
        if !key.starts_with("term.") {
            assert!(zh.contains_key(key), "missing Chinese key {key}");
        }
        if let Some(translated) = zh.get(key) {
            assert!(!translated.trim().is_empty(), "empty Chinese value {key}");
            assert_eq!(parameters(text), parameters(translated), "{key}");
        }
    }
    for key in zh.keys() {
        assert!(en.contains_key(key), "unknown Chinese key {key}");
    }
}

#[test]
fn i18n_fallback_and_override_use_source_owned_fixtures() {
    let (en, _) = catalogs();
    let key = "term.mode.plan";
    let mut target = Catalog::new();
    assert_eq!(
        lookup(Locale::ZhCn, key, en, &target),
        Some(en[key].as_str())
    );
    let override_text = format!("override: {}", en[key]);
    target.insert(key.into(), override_text.clone());
    assert_eq!(
        lookup(Locale::ZhCn, key, en, &target),
        Some(override_text.as_str())
    );
}

#[test]
fn i18n_interpolation_is_single_pass_and_unknown_keys_never_leak() {
    let (en, _) = catalogs();
    assert_eq!(
        tr_in(Locale::En, "session.deleted", &[("id", "{path}")]),
        en["session.deleted"].replace("{id}", "{path}")
    );
    assert_eq!(
        tr_in(Locale::En, "does.not.exist", &[]),
        en["error.unknown"]
    );
    assert_eq!(
        tr_in(Locale::En, "session.deleted", &[]),
        en["error.unknown"]
    );
}

#[test]
fn i18n_defaults_to_english_until_initialized() {
    assert_eq!(current_locale(), Locale::En);
    assert_eq!(tr("error.unknown", &[]), catalogs().0["error.unknown"]);
}

#[test]
fn i18n_language_tag_normalization() {
    for tag in ["zh", "zh_CN.UTF-8", "zh-Hant", " ZH-tw "] {
        assert_eq!(Locale::from_language_tag(tag), Locale::ZhCn);
    }
    for tag in ["en-US", "C", "POSIX", "fr_FR", ""] {
        assert_eq!(Locale::from_language_tag(tag), Locale::En);
    }
}
