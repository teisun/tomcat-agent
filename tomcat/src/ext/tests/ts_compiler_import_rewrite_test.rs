//! # NPM import 重写
//!
//! 验证 SWC 转译过程对 npm imports 的处理：
//!
//! - 已知包 (`@mariozechner/pi-tui`、`@mariozechner/pi-coding-agent`、
//!   `@sinclair/typebox`、`@mariozechner/pi-ai`) 重写为 `globalThis.__pi_*`。
//! - `import type { ... }` 完全剥离，不会触发 globalThis 注入。
//! - 未知包 (`unknown-package`) 保留原始 import。
//! - `namespace import` (`import * as`) 与默认 import (`import X from`) 也被
//!   重写到 `globalThis.__pi_*`。

use super::super::ts_compiler::*;

#[test]
fn rewrite_known_npm_imports() {
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
fn default_import_rewritten() {
    let src = r#"
import TypeBox from "@sinclair/typebox";
const t = TypeBox.Type.String();
"#;
    let out = transpile_typescript(src, "test.ts").unwrap();
    assert!(out.contains("globalThis.__pi_typebox"));
    assert!(!out.contains("from \"@sinclair"));
}

#[test]
fn rewrite_tier_a_node_imports() {
    let src = r#"
import path from "node:path";
import { format } from "util";
import { EventEmitter } from "events";
import { Buffer } from "buffer";

const joined = path.join("a", "b");
const msg = format("%s:%s", joined, "ok");
const ee = new EventEmitter();
const buf = Buffer.from(msg);
"#;
    let out = transpile_typescript(src, "test.ts").unwrap();
    assert!(out.contains("globalThis.__node_path"));
    assert!(out.contains("globalThis.__node_util"));
    assert!(out.contains("globalThis.__node_events"));
    assert!(out.contains("globalThis.__node_buffer"));
    assert!(!out.contains("from \"node:path\""));
    assert!(!out.contains("from \"util\""));
    assert!(!out.contains("from \"events\""));
    assert!(!out.contains("from \"buffer\""));
}
