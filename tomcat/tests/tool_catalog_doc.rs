use tomcat::core::tools::contract::catalog::render_tool_catalog_markdown;

#[test]
fn committed_tool_catalog_matches_catalog_renderer() {
    let expected = render_tool_catalog_markdown();
    let actual =
        std::fs::read_to_string("docs/tool-catalog.md").expect("docs/tool-catalog.md must exist");

    assert_eq!(
        actual,
        expected,
        "docs/tool-catalog.md is out of date; run UPDATE_TOOL_CATALOG=1 cargo run --bin gen-tool-catalog"
    );
}

#[test]
fn bash_and_clippy_docs_preserve_current_contracts() {
    let bash = std::fs::read_to_string("docs/architecture/tools/bash.md").unwrap();
    assert!(bash.contains("foreground_wait_ms = 16000"));
    assert!(bash.contains("窗口到期不是终态"));
    assert!(bash.contains("task_stop"));
    for obsolete in ["timed_out=true", "默认 120s", "超时杀进程"] {
        assert!(
            !bash.contains(obsolete),
            "obsolete bash contract remains: {obsolete}"
        );
    }

    let idioms =
        std::fs::read_to_string("docs/openspec/specs/guides/coding/RUST_IDIOMS_SPEC.md").unwrap();
    let acceptance =
        std::fs::read_to_string("docs/agents/INTEGRATION_MERGE_AND_ACCEPTANCE.md").unwrap();
    for doc in [&idioms, &acceptance] {
        assert!(doc.contains("cargo clippy -p tomcat --lib -- -D warnings"));
        assert!(doc.contains("cargo clippy --all-targets -- -D warnings"));
    }
}
