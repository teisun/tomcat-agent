use pi_wasm::core::tools::contract::catalog::render_tool_catalog_markdown;

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
