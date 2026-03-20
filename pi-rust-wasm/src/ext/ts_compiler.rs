//! TypeScript → JavaScript（SWC strip），供插件零构建加载路径使用。
//! 参考 pi_agent_rust `transpile_typescript_module`；QuickJS 脚本模式需去掉 `export default`。

use crate::infra::error::AppError;
use std::path::Path;
use swc_common::{sync::Lrc, FileName, Globals, Mark, SourceMap, GLOBALS};
use swc_ecma_ast::{Module as SwcModule, Pass, Program as SwcProgram};
use swc_ecma_codegen::{text_writer::JsWriter, Emitter};
use swc_ecma_parser::{Parser as SwcParser, StringInput, Syntax, TsSyntax};
use swc_ecma_transforms_base::resolver;
use swc_ecma_transforms_typescript::strip;

/// 将 TypeScript 模块源码转译为 ES 模块风格 JS（仍含 `import` / `export` 时由调用方处理）。
pub fn transpile_typescript(source: &str, filename: &str) -> Result<String, AppError> {
    let globals = Globals::new();
    GLOBALS.set(&globals, || transpile_typescript_inner(source, filename))
}

fn transpile_typescript_inner(source: &str, filename: &str) -> Result<String, AppError> {
    let cm: Lrc<SourceMap> = Lrc::default();
    let fm = cm.new_source_file(
        FileName::Custom(filename.to_string()).into(),
        source.to_string(),
    );

    let tsx = Path::new(filename)
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("tsx"));
    let syntax = Syntax::Typescript(TsSyntax {
        tsx,
        decorators: true,
        ..Default::default()
    });

    let mut parser = SwcParser::new(syntax, StringInput::from(&*fm), None);
    let module: SwcModule = parser
        .parse_module()
        .map_err(|e| AppError::Plugin(format!("TS parse {filename}: {e:?}")))?;

    let unresolved_mark = Mark::new();
    let top_level_mark = Mark::new();
    let mut program = SwcProgram::Module(module);
    {
        let mut pass = resolver(unresolved_mark, top_level_mark, false);
        pass.process(&mut program);
    }
    {
        let mut pass = strip(unresolved_mark, top_level_mark);
        pass.process(&mut program);
    }
    let SwcProgram::Module(module) = program else {
        return Err(AppError::Plugin(format!(
            "TS transpile {filename}: expected module after strip"
        )));
    };

    let mut buf = Vec::new();
    {
        let mut emitter = Emitter {
            cfg: swc_ecma_codegen::Config::default(),
            comments: None,
            cm: cm.clone(),
            wr: JsWriter::new(cm, "\n", &mut buf, None),
        };
        emitter
            .emit_module(&module)
            .map_err(|e| AppError::Plugin(format!("TS emit {filename}: {e}")))?;
    }

    String::from_utf8(buf).map_err(|e| AppError::Plugin(format!("TS utf8 {filename}: {e}")))
}

/// 转译 pi-mono 风格 `export default function (pi) { ... }` 插件，并追加对 `globalThis.pi` 的调用（QuickJS 脚本入口）。
pub fn transpile_pi_plugin_for_quickjs(source: &str, filename: &str) -> Result<String, AppError> {
    let js = transpile_typescript(source, filename)?;
    Ok(wrap_export_default_pi_plugin(&js))
}

fn wrap_export_default_pi_plugin(js: &str) -> String {
    let needle = "export default function";
    if let Some(pos) = js.find(needle) {
        let mut out = String::with_capacity(js.len() + 128);
        out.push_str(&js[..pos]);
        out.push_str("function __pi_plugin_default");
        out.push_str(&js[pos + needle.len()..]);
        out.push_str(
            "\nif (typeof globalThis.pi !== 'undefined') { __pi_plugin_default(globalThis.pi); }\n",
        );
        out
    } else {
        js.to_string()
    }
}

#[cfg(test)]
mod tests {
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
        let src = include_str!("../../tests/fixtures/pi_mono_tps/tps.ts");
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
}
