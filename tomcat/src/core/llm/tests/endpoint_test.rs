use super::super::endpoint::{build_path_aware_endpoint, gemini_api_root};

#[test]
fn adds_v1_for_bare_hosts() {
    assert_eq!(
        build_path_aware_endpoint("https://api.openai.com", "responses"),
        "https://api.openai.com/v1/responses"
    );
    assert_eq!(
        build_path_aware_endpoint("https://api.example.test", "chat/completions"),
        "https://api.example.test/v1/chat/completions"
    );
    assert_eq!(
        build_path_aware_endpoint("https://api.example.test", "messages"),
        "https://api.example.test/v1/messages"
    );
}

#[test]
fn preserves_explicit_provider_paths() {
    assert_eq!(
        build_path_aware_endpoint("https://open.bigmodel.cn/api/paas/v4/", "chat/completions",),
        "https://open.bigmodel.cn/api/paas/v4/chat/completions"
    );
    assert_eq!(
        build_path_aware_endpoint("https://api.anthropic.com/v1", "messages"),
        "https://api.anthropic.com/v1/messages"
    );
}

#[test]
fn gemini_api_root_normalizes_bare_v1_prerelease_and_gateway_prefixes() {
    assert_eq!(
        gemini_api_root("https://hk.fcodex.top"),
        "https://hk.fcodex.top/v1beta"
    );
    assert_eq!(
        gemini_api_root("https://relay.example.com/v1/"),
        "https://relay.example.com/v1beta"
    );
    assert_eq!(
        gemini_api_root("https://generativelanguage.googleapis.com/v1beta/"),
        "https://generativelanguage.googleapis.com/v1beta"
    );
    assert_eq!(
        gemini_api_root("https://generativelanguage.googleapis.com/v1alpha"),
        "https://generativelanguage.googleapis.com/v1alpha"
    );
    assert_eq!(
        gemini_api_root("https://gw.example.com/acct/google-ai-studio"),
        "https://gw.example.com/acct/google-ai-studio/v1beta"
    );
    assert_eq!(
        gemini_api_root("https://v1.example.com"),
        "https://v1.example.com/v1beta"
    );
}
