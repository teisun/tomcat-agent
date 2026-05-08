//! # TypeScript → JavaScript 转译路径
//!
//! 验证 `transpile_typescript` / `transpile_pi_plugin_for_quickjs` 输出形态：
//!
//! - `transpile_strips_type_annotations`：剥离 `: number` 等类型标注。
//! - `transpile_pi_plugin_wraps_default_export`：默认导出被改写为
//!   `__pi_plugin_default` 并挂到 `globalThis.pi`。
//! - `transpile_pi_mono_tps_fixture`：以仓库自带 `pi_mono_tps/tps.ts` 为输入，
//!   验证 `import type` 与 `@mariozechner/...` 被剥离 / 重写。
//! - `named_default_export_stripped`：命名 default export 的原函数名被去掉，
//!   只保留 `__pi_plugin_default(`。

use super::super::ts_compiler::*;

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
