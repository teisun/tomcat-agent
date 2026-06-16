use crate::ext::plugin::parse_manifest;
use crate::ext::ts_compiler::{transpile_pi_plugin_for_quickjs, transpile_typescript};
use crate::infra::error::AppError;
use std::collections::HashSet;
use std::fs;
use std::path::{Component, Path, PathBuf};
use swc_common::{sync::Lrc, FileName, SourceMap};
use swc_ecma_ast::ModuleItem;
use swc_ecma_parser::{EsSyntax, Parser as SwcParser, StringInput, Syntax};

const SOURCE_EXTENSIONS: &[&str] = &["js", "ts", "tsx"];
const ENTRY_CANDIDATES: &[&str] = &["index.js", "index.ts", "index.tsx"];
const ROOT_PRIORITY_STEMS: &[&str] = &["config", "shared", "parsers"];

#[derive(Debug, Clone)]
pub struct PluginBundleResult {
    pub plugin_root: PathBuf,
    pub src_dir: PathBuf,
    pub output_path: PathBuf,
    pub sources: Vec<PathBuf>,
    pub output: String,
}

pub fn bundle_plugin_from_path(path: impl AsRef<Path>) -> Result<PluginBundleResult, AppError> {
    let (plugin_root, manifest_main) = resolve_manifest_and_root(path.as_ref())?;
    let src_dir = plugin_root.join("src");
    if !src_dir.is_dir() {
        return Err(AppError::Plugin(format!(
            "插件源码目录不存在: {}",
            src_dir.display()
        )));
    }

    let output_path = resolve_output_path(&plugin_root, &src_dir, &manifest_main)?;
    let sources = ordered_source_files(&src_dir)?;
    let output = render_bundle(&plugin_root, &sources)?;

    Ok(PluginBundleResult {
        plugin_root,
        src_dir,
        output_path,
        sources,
        output,
    })
}

pub fn write_plugin_bundle_from_path(
    path: impl AsRef<Path>,
) -> Result<PluginBundleResult, AppError> {
    let result = bundle_plugin_from_path(path)?;
    if let Some(parent) = result.output_path.parent() {
        fs::create_dir_all(parent).map_err(AppError::Io)?;
    }
    fs::write(&result.output_path, &result.output).map_err(AppError::Io)?;
    Ok(result)
}

fn resolve_manifest_and_root(path: &Path) -> Result<(PathBuf, String), AppError> {
    let manifest_path = if path.is_dir() {
        path.join("plugin.json")
    } else if path.file_name().is_some_and(|name| name == "plugin.json") {
        path.to_path_buf()
    } else {
        return Err(AppError::Plugin(format!(
            "请传入插件目录或 plugin.json 路径: {}",
            path.display()
        )));
    };

    let plugin_root = manifest_path.parent().ok_or_else(|| {
        AppError::Plugin(format!("无法解析插件根目录: {}", manifest_path.display()))
    })?;
    let raw = fs::read_to_string(&manifest_path).map_err(AppError::Io)?;
    let manifest = parse_manifest(&raw)?;
    Ok((plugin_root.to_path_buf(), manifest.main))
}

fn resolve_output_path(
    plugin_root: &Path,
    src_dir: &Path,
    manifest_main: &str,
) -> Result<PathBuf, AppError> {
    let rel = normalize_relative_path(manifest_main)?;
    let output_path = plugin_root.join(&rel);
    let ext = output_path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or_default();
    if ext != "js" {
        return Err(AppError::Plugin(format!(
            "plugin.json.main 必须指向 .js 产物，当前为: {}",
            manifest_main
        )));
    }
    if output_path.starts_with(src_dir) {
        return Err(AppError::Plugin(format!(
            "构建产物不能写回 src/ 目录内: {}",
            output_path.display()
        )));
    }
    Ok(output_path)
}

fn normalize_relative_path(raw: &str) -> Result<PathBuf, AppError> {
    let rel = Path::new(raw);
    if rel.is_absolute() {
        return Err(AppError::Plugin(format!(
            "不允许绝对路径 main 输出: {}",
            raw
        )));
    }

    let mut normalized = PathBuf::new();
    for component in rel.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(part) => normalized.push(part),
            Component::ParentDir => {
                if !normalized.pop() {
                    return Err(AppError::Plugin(format!(
                        "main 路径不能逃出插件根目录: {}",
                        raw
                    )));
                }
            }
            Component::RootDir | Component::Prefix(_) => {
                return Err(AppError::Plugin(format!("非法 main 路径: {}", raw)));
            }
        }
    }

    if normalized.as_os_str().is_empty() {
        return Err(AppError::Plugin("plugin.json.main 不能为空".to_string()));
    }

    Ok(normalized)
}

fn ordered_source_files(src_dir: &Path) -> Result<Vec<PathBuf>, AppError> {
    let entry = resolve_entry_path(src_dir)?;
    let mut all_sources = Vec::new();
    collect_source_files(src_dir, &mut all_sources)?;
    if all_sources.is_empty() {
        return Err(AppError::Plugin(format!(
            "src/ 目录内没有可构建的源码文件: {}",
            src_dir.display()
        )));
    }

    let mut seen = HashSet::new();
    let mut ordered = Vec::new();

    for stem in ROOT_PRIORITY_STEMS {
        if let Some(path) = find_root_source_by_stem(src_dir, stem)? {
            seen.insert(path.clone());
            ordered.push(path);
        }
    }

    let mut remaining = all_sources
        .into_iter()
        .filter(|path| path != &entry && !seen.contains(path))
        .collect::<Vec<_>>();
    remaining.sort_by_key(|path| relative_key(src_dir, path));
    ordered.extend(remaining);
    ordered.push(entry);
    Ok(ordered)
}

fn resolve_entry_path(src_dir: &Path) -> Result<PathBuf, AppError> {
    let matches = ENTRY_CANDIDATES
        .iter()
        .map(|name| src_dir.join(name))
        .filter(|path| path.is_file())
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [] => Err(AppError::Plugin(format!(
            "src/ 缺少入口文件，期望其一: {}",
            ENTRY_CANDIDATES.join(", ")
        ))),
        [single] => Ok(single.clone()),
        _ => Err(AppError::Plugin(format!(
            "src/ 存在多个入口文件，请只保留一个: {}",
            matches
                .iter()
                .map(|path| relative_key(src_dir, path))
                .collect::<Vec<_>>()
                .join(", ")
        ))),
    }
}

fn find_root_source_by_stem(src_dir: &Path, stem: &str) -> Result<Option<PathBuf>, AppError> {
    let matches = SOURCE_EXTENSIONS
        .iter()
        .map(|ext| src_dir.join(format!("{stem}.{ext}")))
        .filter(|path| path.is_file())
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [] => Ok(None),
        [single] => Ok(Some(single.clone())),
        _ => Err(AppError::Plugin(format!(
            "src/ 根目录存在多个 `{stem}` 源文件，请只保留一个扩展名版本"
        ))),
    }
}

fn collect_source_files(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), AppError> {
    let mut entries = fs::read_dir(dir)
        .map_err(AppError::Io)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(AppError::Io)?;
    entries.sort_by_key(|entry| entry.path());

    for entry in entries {
        let path = entry.path();
        if path.is_dir() {
            collect_source_files(&path, out)?;
            continue;
        }
        if is_supported_source_file(&path) {
            out.push(path);
        }
    }
    Ok(())
}

fn is_supported_source_file(path: &Path) -> bool {
    let Some(ext) = path.extension().and_then(|ext| ext.to_str()) else {
        return false;
    };
    SOURCE_EXTENSIONS.contains(&ext) && !path.to_string_lossy().ends_with(".d.ts")
}

fn render_bundle(plugin_root: &Path, sources: &[PathBuf]) -> Result<String, AppError> {
    let mut output = String::from(
        "// Generated by `tomcat plugin build`.\n// Edit files under `src/` and rebuild.\n",
    );

    for path in sources {
        let rel = relative_key(plugin_root, path);
        let is_entry = path.file_stem().is_some_and(|stem| stem == "index")
            && path
                .parent()
                .is_some_and(|parent| parent == plugin_root.join("src"));
        let compiled = compile_source_for_bundle(path, &rel, is_entry)?;
        output.push('\n');
        output.push_str("// --- ");
        output.push_str(&rel);
        output.push_str(" ---\n");
        output.push_str(&compiled);
        if !compiled.ends_with('\n') {
            output.push('\n');
        }
    }

    Ok(output)
}

fn compile_source_for_bundle(
    path: &Path,
    display_name: &str,
    is_entry: bool,
) -> Result<String, AppError> {
    let raw = fs::read_to_string(path).map_err(AppError::Io)?;
    let ext = path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or_default();
    let compiled = match ext {
        "ts" | "tsx" if is_entry => transpile_pi_plugin_for_quickjs(&raw, display_name)?,
        "ts" | "tsx" => transpile_typescript(&raw, display_name)?,
        "js" => raw,
        _ => {
            return Err(AppError::Plugin(format!(
                "不支持的源码扩展名: {}",
                path.display()
            )))
        }
    };

    reject_leftover_module_syntax(display_name, &compiled)?;
    Ok(compiled)
}

fn reject_leftover_module_syntax(display_name: &str, source: &str) -> Result<(), AppError> {
    let cm: Lrc<SourceMap> = Lrc::default();
    let fm = cm.new_source_file(
        FileName::Custom(display_name.to_string()).into(),
        source.to_string(),
    );
    let mut parser = SwcParser::new(
        Syntax::Es(EsSyntax {
            jsx: true,
            ..Default::default()
        }),
        StringInput::from(&*fm),
        None,
    );
    let module = parser
        .parse_module()
        .map_err(|err| AppError::Plugin(format!("bundle 解析失败（{display_name}）: {err:?}")))?;
    if module
        .body
        .iter()
        .any(|item| matches!(item, ModuleItem::ModuleDecl(_)))
    {
        return Err(AppError::Plugin(format!(
            "构建后的源码仍包含 import/export，当前 bundle 仅支持脚本片段拼接: {display_name}"
        )));
    }
    Ok(())
}

fn relative_key(base: &Path, path: &Path) -> String {
    path.strip_prefix(base)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}
