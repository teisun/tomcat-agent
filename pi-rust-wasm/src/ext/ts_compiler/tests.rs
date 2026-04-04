use super::*;

#[test]
fn transpile_strips_type_annotations() {
    let js = transpile_typescript("const x: number = 1;\nexport {};\n", "x.ts").unwrap();
    assert!(!js.contains(": number"));
    assert!(js.contains("1"));
}

#[test]
fn transpile_pi_plugin_wraps_default_export() {
    let src = r#"export default function (pi: unknown) { pi; }
"#;
    let out = transpile_pi_plugin_for_quickjs(src, "p.ts").unwrap();
    assert!(!out.contains("export default"));
    assert!(out.contains("__pi_plugin_default"));
    assert!(out.contains("globalThis.pi"));
}

#[test]
fn transpile_pi_mono_tps_fixture() {
    let src = include_str!("../../../tests/fixtures/pi_mono_tps/tps.ts");
    let out = transpile_pi_plugin_for_quickjs(src, "tps.ts").unwrap();
    assert!(
        !out.contains("ExtensionAPI") && !out.contains("@mariozechner"),
        "类型与 import type 应被剥离"
    );
    assert!(
        out.contains("__pi_plugin_default") || out.contains("function"),
        "应产出可执行插件脚本"
    );
}

#[test]
fn rewrite_known_npm_imports() {
    // All imported names must be referenced in code body — SWC strip removes unused import specifiers.
    let src = r#"
import { Container, Key, matchesKey, SelectList, Text } from "@mariozechner/pi-tui";
import { DynamicBorder } from "@mariozechner/pi-coding-agent";
const c = new Container();
const k = Key.escape;
const m = matchesKey("a", "b");
const s = new SelectList([], 10, {});
const t = new Text("hi");
const d = new DynamicBorder();
"#;
    let out = transpile_typescript(src, "test.ts").unwrap();
    assert!(
        !out.contains("from \"@mariozechner"),
        "known npm imports should be rewritten, got:\n{out}"
    );
    assert!(
        out.contains("globalThis.__pi_tui"),
        "should reference globalThis.__pi_tui, got:\n{out}"
    );
    assert!(
        out.contains("globalThis.__pi_coding_agent"),
        "should reference globalThis.__pi_coding_agent, got:\n{out}"
    );
    assert!(
        out.contains("Container"),
        "should preserve binding names, got:\n{out}"
    );
}

#[test]
fn rewrite_typebox_and_pi_ai_imports() {
    let src = r#"
import { Type } from "@sinclair/typebox";
import { StringEnum, complete } from "@mariozechner/pi-ai";
const schema = Type.String();
const e = StringEnum(["a", "b"]);
complete("model", []);
"#;
    let out = transpile_typescript(src, "test.ts").unwrap();
    assert!(out.contains("globalThis.__pi_typebox"));
    assert!(out.contains("globalThis.__pi_ai"));
    assert!(!out.contains("from \"@sinclair"));
    assert!(!out.contains("from \"@mariozechner/pi-ai"));
}

#[test]
fn import_type_stripped_not_rewritten() {
    let src = r#"
import type { ExtensionAPI } from "@mariozechner/pi-mono";
const x = 1;
"#;
    let out = transpile_typescript(src, "test.ts").unwrap();
    assert!(
        !out.contains("ExtensionAPI"),
        "import type should be stripped entirely"
    );
    assert!(!out.contains("globalThis.__"));
}

#[test]
fn unknown_package_imports_preserved() {
    let src = r#"
import { Foo } from "unknown-package";
const x = Foo;
"#;
    let out = transpile_typescript(src, "test.ts").unwrap();
    assert!(
        out.contains("\"unknown-package\""),
        "unknown imports should remain, got:\n{out}"
    );
}

#[test]
fn namespace_import_rewritten() {
    let src = r#"
import * as tui from "@mariozechner/pi-tui";
const c = new tui.Container();
"#;
    let out = transpile_typescript(src, "test.ts").unwrap();
    assert!(
        out.contains("globalThis.__pi_tui"),
        "namespace import should reference globalThis, got:\n{out}"
    );
    assert!(
        !out.contains("from \"@mariozechner"),
        "should not contain original import, got:\n{out}"
    );
}

#[test]
fn named_default_export_stripped() {
    let src = r#"export default function myPlugin(pi: unknown) { pi; }
"#;
    let out = transpile_pi_plugin_for_quickjs(src, "p.ts").unwrap();
    assert!(
        !out.contains("myPlugin"),
        "original function name should be stripped, got:\n{out}"
    );
    assert!(out.contains("function __pi_plugin_default("));
}

#[test]
fn default_import_rewritten() {
    let src = r#"
import TypeBox from "@sinclair/typebox";
const t = TypeBox.Type.String();
"#;
    let out = transpile_typescript(src, "test.ts").unwrap();
    assert!(out.contains("globalThis.__pi_typebox"));
    assert!(!out.contains("from \"@sinclair"));
}
