//! # NPM import 重写
//!
//! 验证 SWC 转译过程对 npm imports 的处理：
//!
//! - 已支持的小型工具包（`@sinclair/typebox`、`ms`）与 Tier-A Node alias
//!   重写为 `globalThis.__*`。
//! - `import type { ... }` 完全剥离，不会触发 globalThis 注入。
//! - 未支持的 legacy pi-mono 包与其它未知包保留原始 import。
//! - 默认 import (`import X from`) 也会重写到 `globalThis.__pi_*`。

use super::super::ts_compiler::*;

#[test]
fn rewrite_supported_utility_imports() {
    let src = r#"
import { Type } from "@sinclair/typebox";
import ms from "ms";
const schema = Type.Object({ timeout: Type.String() });
const timeout = ms("5m");
"#;
    let out = transpile_typescript(src, "test.ts").unwrap();
    assert!(
        !out.contains("from \"@sinclair/typebox\""),
        "supported utility imports should be rewritten, got:\n{out}"
    );
    assert!(
        out.contains("globalThis.__pi_typebox"),
        "should reference globalThis.__pi_typebox, got:\n{out}"
    );
    assert!(
        out.contains("globalThis.__pi_ms"),
        "should reference globalThis.__pi_ms, got:\n{out}"
    );
    assert!(
        out.contains("Type.Object"),
        "should preserve binding names, got:\n{out}"
    );
}

#[test]
fn unsupported_legacy_pi_mono_imports_are_preserved() {
    let src = r#"
import { Container } from "@mariozechner/pi-tui";
import { DynamicBorder } from "@mariozechner/pi-coding-agent";
import { StringEnum } from "@mariozechner/pi-ai";
import { SandboxManager } from "@anthropic-ai/sandbox-runtime";
const items = [Container, DynamicBorder, StringEnum, SandboxManager];
"#;
    let out = transpile_typescript(src, "test.ts").unwrap();
    assert!(out.contains("\"@mariozechner/pi-tui\""));
    assert!(out.contains("\"@mariozechner/pi-coding-agent\""));
    assert!(out.contains("\"@mariozechner/pi-ai\""));
    assert!(out.contains("\"@anthropic-ai/sandbox-runtime\""));
    assert!(
        !out.contains("globalThis.__pi_tui")
            && !out.contains("globalThis.__pi_coding_agent")
            && !out.contains("globalThis.__pi_ai")
            && !out.contains("globalThis.__pi_sandbox_runtime"),
        "legacy pi-mono imports should remain explicit and unsupported, got:\n{out}"
    );
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
