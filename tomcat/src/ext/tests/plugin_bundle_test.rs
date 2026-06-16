use super::super::plugin_bundle::{bundle_plugin_from_path, write_plugin_bundle_from_path};

fn plugin_fixture(manifest_main: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(
        dir.path().join("plugin.json"),
        format!(
            r#"{{
  "id": "bundle-test",
  "name": "bundle-test",
  "version": "0.1.0",
  "description": "bundle test",
  "author": "tests",
  "main": "{manifest_main}",
  "requiredPermissions": [],
  "requiredApiVersion": "1.0",
  "tags": []
}}"#
        ),
    )
    .expect("write plugin.json");
    dir
}

#[test]
fn bundle_concatenates_sources_in_deterministic_order() {
    let dir = plugin_fixture("main.js");
    let src = dir.path().join("src");
    std::fs::create_dir_all(src.join("backends")).expect("create backends dir");
    std::fs::write(src.join("config.js"), "var CONFIG_READY = true;\n").expect("write config");
    std::fs::write(
        src.join("shared.js"),
        "function helper() { return CONFIG_READY; }\n",
    )
    .expect("write shared");
    std::fs::write(
        src.join("backends").join("zeta.js"),
        "function zeta() { return helper(); }\n",
    )
    .expect("write backend");
    std::fs::write(
        src.join("index.js"),
        "pi.registerFunction('x', function () { return zeta(); });\n",
    )
    .expect("write index");

    let first = bundle_plugin_from_path(dir.path()).expect("first bundle");
    let second = bundle_plugin_from_path(dir.path()).expect("second bundle");

    assert_eq!(first.output, second.output);
    let config_pos = first
        .output
        .find("// --- src/config.js ---")
        .expect("config banner");
    let shared_pos = first
        .output
        .find("// --- src/shared.js ---")
        .expect("shared banner");
    let backend_pos = first
        .output
        .find("// --- src/backends/zeta.js ---")
        .expect("backend banner");
    let index_pos = first
        .output
        .find("// --- src/index.js ---")
        .expect("index banner");
    assert!(config_pos < shared_pos && shared_pos < backend_pos && backend_pos < index_pos);
}

#[test]
fn bundle_strips_typescript_via_ts_compiler() {
    let dir = plugin_fixture("main.js");
    let src = dir.path().join("src");
    std::fs::create_dir_all(&src).expect("create src");
    std::fs::write(src.join("config.ts"), "var answer: number = 1;\n").expect("write config.ts");
    std::fs::write(
        src.join("index.ts"),
        "export default function (pi: unknown) { pi.registerFunction('x', function () { return answer; }); }\n",
    )
    .expect("write index.ts");

    let bundle = bundle_plugin_from_path(dir.path()).expect("bundle ts");
    assert!(!bundle.output.contains(": number"));
    assert!(bundle.output.contains("function __pi_plugin_default"));
    assert!(bundle.output.contains("pi.registerFunction('x'"));
}

#[test]
fn bundle_errors_on_missing_src_dir() {
    let dir = plugin_fixture("main.js");
    let err = bundle_plugin_from_path(dir.path()).expect_err("missing src should fail");
    assert!(err.to_string().contains("插件源码目录不存在"));
}

#[test]
fn bundle_errors_on_missing_entry() {
    let dir = plugin_fixture("main.js");
    let src = dir.path().join("src");
    std::fs::create_dir_all(&src).expect("create src");
    std::fs::write(src.join("config.js"), "var ready = true;\n").expect("write config");

    let err = bundle_plugin_from_path(dir.path()).expect_err("missing entry should fail");
    assert!(err.to_string().contains("缺少入口文件"));
}

#[test]
fn bundle_errors_on_syntax_error_with_filename() {
    let dir = plugin_fixture("main.js");
    let src = dir.path().join("src");
    std::fs::create_dir_all(&src).expect("create src");
    std::fs::write(src.join("config.js"), "var ready = ;\n").expect("write config");
    std::fs::write(
        src.join("index.js"),
        "pi.registerFunction('x', function () {});\n",
    )
    .expect("write index");

    let err = bundle_plugin_from_path(dir.path()).expect_err("syntax error should fail");
    assert!(err.to_string().contains("src/config.js"));
}

#[test]
fn build_refuses_paths_outside_plugin_dir() {
    let dir = plugin_fixture("../escape.js");
    let src = dir.path().join("src");
    std::fs::create_dir_all(&src).expect("create src");
    std::fs::write(
        src.join("index.js"),
        "pi.registerFunction('x', function () {});\n",
    )
    .expect("write index");

    let err = write_plugin_bundle_from_path(dir.path()).expect_err("sandbox rejection");
    assert!(err.to_string().contains("逃出插件根目录"));
}
