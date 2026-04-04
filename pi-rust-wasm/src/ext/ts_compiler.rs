//! TypeScript → JavaScript（SWC strip），供插件零构建加载路径使用。
//! 参考 pi_agent_rust `transpile_typescript_module`；QuickJS 脚本模式需去掉 `export default`。
//! 与 `load_plugin` / `init_vm` 组合脚本的数据流见 openspec：
//! `architecture/plugin-system/pi-mono-compat-strategy.md` §「TASK-05b Tier1：加载与事件链路」。

use crate::infra::error::AppError;
use std::path::Path;
use swc_common::{sync::Lrc, FileName, Globals, Mark, SourceMap, SyntaxContext, DUMMY_SP, GLOBALS};
use swc_ecma_ast::{
    AssignPatProp, BindingIdent, Decl, Expr, Ident, IdentName, ImportSpecifier, KeyValuePatProp,
    MemberExpr, MemberProp, Module as SwcModule, ModuleDecl, ModuleExportName, ModuleItem,
    ObjectPat, ObjectPatProp, Pass, Pat, Program as SwcProgram, PropName, Stmt, VarDecl,
    VarDeclKind, VarDeclarator,
};
use swc_ecma_codegen::{text_writer::JsWriter, Emitter};
use swc_ecma_parser::{Parser as SwcParser, StringInput, Syntax, TsSyntax};
use swc_ecma_transforms_base::resolver;
use swc_ecma_transforms_typescript::strip;

/// Known npm package → globalThis property mapping for QuickJS script-mode import rewriting.
/// Each package has a corresponding `assets/js/<name>_shim.js` injected by `build_combined_script`.
const NPM_SHIM_MAP: &[(&str, &str)] = &[
    ("@mariozechner/pi-tui", "__pi_tui"),
    ("@mariozechner/pi-coding-agent", "__pi_coding_agent"),
    ("@mariozechner/pi-ai", "__pi_ai"),
    ("@sinclair/typebox", "__pi_typebox"),
    // Node.js built-in modules
    ("fs", "__node_fs"),
    ("node:fs", "__node_fs"),
    ("fs/promises", "__node_fs_promises"),
    ("node:fs/promises", "__node_fs_promises"),
    ("path", "__node_path"),
    ("node:path", "__node_path"),
    ("child_process", "__node_child_process"),
    ("node:child_process", "__node_child_process"),
    ("os", "__node_os"),
    ("node:os", "__node_os"),
    ("crypto", "__node_crypto"),
    ("node:crypto", "__node_crypto"),
    // External npm packages
    ("@anthropic-ai/sandbox-runtime", "__pi_sandbox_runtime"),
    ("ms", "__pi_ms"),
    // Subagent local import
    ("./agents.js", "__pi_subagent_agents"),
];

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
    let SwcProgram::Module(mut module) = program else {
        return Err(AppError::Plugin(format!(
            "TS transpile {filename}: expected module after strip"
        )));
    };

    rewrite_npm_imports(&mut module);

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
        let after = &js[pos + needle.len()..];
        // Skip optional original function name so
        // `export default function myName(` becomes `function __pi_plugin_default(`
        let after = after.trim_start();
        let skip_name = if after.starts_with('(') {
            after
        } else if let Some(paren) = after.find('(') {
            &after[paren..]
        } else {
            after
        };
        let mut out = String::with_capacity(js.len() + 128);
        out.push_str(&js[..pos]);
        out.push_str("function __pi_plugin_default");
        out.push_str(skip_name);
        out.push_str(
            "\nif (typeof globalThis.pi !== 'undefined') { __pi_plugin_default(globalThis.pi); }\n",
        );
        out
    } else {
        js.to_string()
    }
}

// ---------------------------------------------------------------------------
// npm import → globalThis rewrite (QuickJS script mode)
// ---------------------------------------------------------------------------

/// Replace `import { X } from "<known-pkg>"` with `var { X } = globalThis.__xxx;`
/// for packages listed in [`NPM_SHIM_MAP`]. Other imports pass through unchanged.
fn rewrite_npm_imports(module: &mut SwcModule) {
    let shim_map: Vec<(&str, &str)> = NPM_SHIM_MAP.to_vec();
    let old_body = std::mem::take(&mut module.body);
    let mut new_body = Vec::with_capacity(old_body.len());

    for item in old_body {
        if let ModuleItem::ModuleDecl(ModuleDecl::Import(ref import_decl)) = item {
            if import_decl.type_only {
                continue;
            }
            if let Some(global_prop) = lookup_shim_prop(&shim_map, &import_decl.src) {
                if let Some(var_item) = import_to_globalthis_var(import_decl, global_prop) {
                    new_body.push(var_item);
                }
                continue;
            }
        }
        new_body.push(item);
    }

    module.body = new_body;
}

/// Byte-level lookup: `Wtf8Atom` doesn't implement `Display`/`Hash<str>`,
/// so compare raw bytes (module specifiers are always valid UTF-8).
fn lookup_shim_prop<'a>(map: &[(&str, &'a str)], src: &swc_ecma_ast::Str) -> Option<&'a str> {
    let bytes = src.value.as_bytes();
    map.iter()
        .find(|(pkg, _)| pkg.as_bytes() == bytes)
        .map(|(_, prop)| *prop)
}

/// Build `var { X, Y } = globalThis.__xxx;` (or `var NS = globalThis.__xxx;` for namespace import).
fn import_to_globalthis_var(
    import_decl: &swc_ecma_ast::ImportDecl,
    global_prop: &str,
) -> Option<ModuleItem> {
    let global_expr = Expr::Member(MemberExpr {
        span: DUMMY_SP,
        obj: Box::new(Expr::Ident(Ident::new_no_ctxt(
            "globalThis".into(),
            DUMMY_SP,
        ))),
        prop: MemberProp::Ident(IdentName::new(global_prop.into(), DUMMY_SP)),
    });

    let mut props: Vec<ObjectPatProp> = Vec::new();
    let mut namespace_local: Option<Ident> = None;

    for spec in &import_decl.specifiers {
        match spec {
            ImportSpecifier::Named(named) => {
                if named.is_type_only {
                    continue;
                }
                match &named.imported {
                    Some(ModuleExportName::Ident(imported_id)) => {
                        props.push(ObjectPatProp::KeyValue(KeyValuePatProp {
                            key: PropName::Ident(IdentName::new(imported_id.sym.clone(), DUMMY_SP)),
                            value: Box::new(Pat::Ident(BindingIdent {
                                id: named.local.clone(),
                                type_ann: None,
                            })),
                        }));
                    }
                    Some(ModuleExportName::Str(s)) => {
                        props.push(ObjectPatProp::KeyValue(KeyValuePatProp {
                            key: PropName::Str(s.clone()),
                            value: Box::new(Pat::Ident(BindingIdent {
                                id: named.local.clone(),
                                type_ann: None,
                            })),
                        }));
                    }
                    None => {
                        props.push(ObjectPatProp::Assign(AssignPatProp {
                            span: DUMMY_SP,
                            key: BindingIdent {
                                id: named.local.clone(),
                                type_ann: None,
                            },
                            value: None,
                        }));
                    }
                }
            }
            ImportSpecifier::Default(def) => {
                props.push(ObjectPatProp::KeyValue(KeyValuePatProp {
                    key: PropName::Ident(IdentName::new("default".into(), DUMMY_SP)),
                    value: Box::new(Pat::Ident(BindingIdent {
                        id: def.local.clone(),
                        type_ann: None,
                    })),
                }));
            }
            ImportSpecifier::Namespace(ns) => {
                namespace_local = Some(ns.local.clone());
            }
        }
    }

    if let Some(local) = namespace_local {
        return Some(make_var_stmt(
            Pat::Ident(BindingIdent {
                id: local,
                type_ann: None,
            }),
            global_expr,
        ));
    }

    if props.is_empty() {
        return None;
    }

    Some(make_var_stmt(
        Pat::Object(ObjectPat {
            span: DUMMY_SP,
            props,
            optional: false,
            type_ann: None,
        }),
        global_expr,
    ))
}

fn make_var_stmt(name: Pat, init: Expr) -> ModuleItem {
    ModuleItem::Stmt(Stmt::Decl(Decl::Var(Box::new(VarDecl {
        span: DUMMY_SP,
        ctxt: SyntaxContext::empty(),
        kind: VarDeclKind::Var,
        declare: false,
        decls: vec![VarDeclarator {
            span: DUMMY_SP,
            name,
            init: Some(Box::new(init)),
            definite: false,
        }],
    }))))
}

#[cfg(test)]
mod tests;
