use super::super::endpoint::build_path_aware_endpoint;

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
