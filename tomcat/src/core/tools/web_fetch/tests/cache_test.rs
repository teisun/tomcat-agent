use std::time::Duration;

use super::super::cache::{CacheKey, WebFetchCache};
use super::super::types::{WebFetchFormat, WebFetchOutput};
use super::{args, build_runtime, test_config, text_output};

#[tokio::test]
async fn cache_hit_skips_http() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = build_runtime(test_config(), dir.path().join("tool-results"));
    runtime.cache.insert(
        CacheKey::new("https://example.com/cached", "markdown"),
        text_output(
            "https://example.com/cached",
            WebFetchFormat::Markdown,
            "# cached",
        ),
    );

    let output = runtime
        .fetch(args("https://example.com/cached"))
        .await
        .expect("cache hit");
    assert!(output.cached);
    assert_eq!(output.result, "# cached");
}

#[test]
fn cache_key_includes_format() {
    let markdown = CacheKey::new("https://example.com/page", "markdown");
    let text = CacheKey::new("https://example.com/page", "text");
    assert_ne!(markdown, text);
}

#[tokio::test]
async fn cache_miss_after_ttl() {
    let mut cfg = test_config();
    cfg.tools.web_fetch.cache_ttl_secs = 1;
    let cache = WebFetchCache::new(&cfg.tools.web_fetch);
    let key = CacheKey::new("https://example.com/ttl", "markdown");
    cache.insert(
        key.clone(),
        text_output("https://example.com/ttl", WebFetchFormat::Markdown, "ttl"),
    );
    assert!(cache.get(&key).is_some());
    tokio::time::sleep(Duration::from_millis(1_100)).await;
    assert!(cache.get(&key).is_none());
}

#[test]
fn cache_capacity_evicts_oldest() {
    let mut cfg = test_config();
    cfg.tools.web_fetch.cache_capacity_bytes = 500;
    let cache = WebFetchCache::new(&cfg.tools.web_fetch);
    let first = CacheKey::new("https://example.com/one", "markdown");
    let second = CacheKey::new("https://example.com/two", "markdown");
    cache.insert(
        first.clone(),
        text_output(
            "https://example.com/one",
            WebFetchFormat::Markdown,
            &"a".repeat(120),
        ),
    );
    cache.insert(
        second.clone(),
        text_output(
            "https://example.com/two",
            WebFetchFormat::Markdown,
            &"b".repeat(120),
        ),
    );
    cache.run_pending_tasks();

    assert!(
        cache.entry_count() <= 1,
        "over-capacity cache should not retain both large entries"
    );
    assert!(
        cache.get(&first).is_some() || cache.get(&second).is_some(),
        "at least one large entry should remain admitted"
    );
}

#[tokio::test]
async fn cache_hit_without_prompt_drops_cached_prompt_warning() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = build_runtime(test_config(), dir.path().join("tool-results"));
    let mut cached = text_output(
        "https://example.com/cached-warning",
        WebFetchFormat::Markdown,
        "# cached",
    );
    cached.warnings.push("prompt_ignored_mvp".to_string());
    runtime.insert_cached_output_for_test(
        "https://example.com/cached-warning",
        WebFetchFormat::Markdown,
        cached,
    );

    let output = runtime
        .fetch(args("https://example.com/cached-warning"))
        .await
        .expect("cache hit");
    assert!(output.cached);
    assert!(!output
        .warnings
        .iter()
        .any(|warning| warning == "prompt_ignored_mvp"));
}

#[test]
fn redirect_output_is_not_cacheable() {
    let output = WebFetchOutput::redirect(
        "https://example.com/start".to_string(),
        "https://example.com/start".to_string(),
        "https://other.example/landing".to_string(),
        301,
        "Moved Permanently".to_string(),
        12,
        vec!["redirect_off_host".to_string()],
    );
    assert!(!super::super::should_cache(&output));
}
