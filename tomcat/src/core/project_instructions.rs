//! Project prompt files are runtime configuration, not agent tool reads.
//! Only hard Deny blocks them; no grants are created and no confirmation is requested.
use std::collections::BTreeSet;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Component, Path, PathBuf};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_yaml::Value;

use crate::core::permission::{PermissionDecision, PermissionGate};
use crate::core::tools::primitive::PrimitiveOperation;

pub const MAX_FILE_BYTES: usize = 65_536;
const MAX_HEADER_BYTES: usize = 4096;
const MAX_ENTRIES: usize = 4096;
const MAX_FILES: usize = 256;
const MAX_DEPTH: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum InstructionKind {
    Command,
    Skill,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct InstructionCard {
    pub id: String,
    pub kind: InstructionKind,
    pub name: String,
    pub description: String,
    pub source: String,
    pub path: String,
}

#[derive(Debug, Clone)]
pub struct PromptFile {
    pub card: InstructionCard,
    pub file_path: PathBuf,
    pub always_apply: bool,
}

#[derive(Debug, Default)]
pub struct Discovery {
    pub files: Vec<PromptFile>,
    pub diagnostics: Vec<String>,
}

fn diagnostic_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn readable(gate: &dyn PermissionGate, path: &Path) -> Result<(), String> {
    match gate
        .check(PrimitiveOperation::Read, &path.to_string_lossy())
        .map_err(|e| e.to_string())?
    {
        PermissionDecision::Deny { reason } => Err(format!("被 Deny 规则禁止读取：{reason}")),
        _ => Ok(()),
    }
}

/// Reject links in every component below the trusted scope root (including .cursor).
fn safe_path(root: &Path, path: &Path) -> Result<(), String> {
    let relative = path.strip_prefix(root).map_err(|_| "路径超出资源根")?;
    let mut current = root.to_path_buf();
    for component in relative.components() {
        let Component::Normal(name) = component else {
            return Err("非法路径".into());
        };
        current.push(name);
        if fs::symlink_metadata(&current)
            .map_err(|e| e.to_string())?
            .file_type()
            .is_symlink()
        {
            return Err("不读取 symlink".into());
        }
    }
    let canonical = path.canonicalize().map_err(|e| e.to_string())?;
    if !canonical.starts_with(root) {
        return Err("路径超出资源根".into());
    }
    Ok(())
}

pub fn parse_document(text: &str) -> Result<(String, bool, &str), String> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    if text.lines().next() != Some("---") {
        return Ok((String::new(), false, text));
    }
    let (yaml, body) =
        crate::core::skill::frontmatter::split_frontmatter(text).map_err(|e| e.to_string())?;
    if yaml.len() > MAX_HEADER_BYTES {
        return Err("文件头过大".into());
    }
    let value: Value = serde_yaml::from_str(yaml).map_err(|e| format!("YAML 解析失败：{e}"))?;
    let description = value
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or("")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let always = value.get("alwaysApply").and_then(Value::as_bool) == Some(true);
    Ok((description, always, body))
}

/// Bounded, non-interactive runtime read. No client-provided path reaches this helper.
pub fn read_body(root: &Path, path: &Path, gate: &dyn PermissionGate) -> Result<String, String> {
    safe_path(root, path)?;
    readable(gate, path)?;
    let mut file = File::open(path).map_err(|e| e.to_string())?;
    let meta = file.metadata().map_err(|e| e.to_string())?;
    if !meta.is_file() || meta.len() > MAX_FILE_BYTES as u64 {
        return Err("文件不是普通文件或超过 64 KiB".into());
    }
    let mut bytes = Vec::new();
    file.by_ref()
        .take((MAX_FILE_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > MAX_FILE_BYTES {
        return Err("文件超过 64 KiB".into());
    }
    let text = String::from_utf8(bytes).map_err(|_| "文件不是 UTF-8".to_string())?;
    let (_, _, body) = parse_document(&text)?;
    if body.trim().is_empty() {
        return Err("正文为空".into());
    }
    Ok(body.to_string())
}

pub fn discover(
    root: &Path,
    resource_dir: &str,
    rules: bool,
    gate: &dyn PermissionGate,
) -> Discovery {
    let mut result = Discovery::default();
    let Ok(root) = root.canonicalize() else {
        return result;
    };
    let mut sources = vec![".cursor".to_string(), ".agents".to_string()];
    if !sources.iter().any(|s| s == resource_dir) {
        sources.push(resource_dir.to_string());
    }
    let mut seen = BTreeSet::new();
    let mut entries = 0;
    let mut candidates = 0;
    for source in sources {
        let base = root
            .join(&source)
            .join(if rules { "rules" } else { "commands" });
        if !base.exists() {
            continue;
        }
        if let Err(e) = safe_path(&root, &base).and_then(|_| readable(gate, &base)) {
            result
                .diagnostics
                .push(format!("{}：{e}", diagnostic_path(&root, &base)));
            continue;
        }
        if !seen.insert(base.clone()) {
            continue;
        }
        scan(
            &root,
            &base,
            &base,
            &source,
            rules,
            gate,
            0,
            &mut entries,
            &mut candidates,
            &mut result,
        );
    }
    result
}

#[allow(clippy::too_many_arguments)]
fn scan(
    root: &Path,
    base: &Path,
    dir: &Path,
    source: &str,
    rules: bool,
    gate: &dyn PermissionGate,
    depth: usize,
    entries: &mut usize,
    candidates: &mut usize,
    result: &mut Discovery,
) {
    if depth > MAX_DEPTH {
        result
            .diagnostics
            .push(format!("{}：超过 8 层", diagnostic_path(root, dir)));
        return;
    }
    if let Err(e) = safe_path(root, dir).and_then(|_| readable(gate, dir)) {
        result
            .diagnostics
            .push(format!("{}：{e}", diagnostic_path(root, dir)));
        return;
    }
    let listing = match fs::read_dir(dir) {
        Ok(v) => v,
        Err(e) => {
            result
                .diagnostics
                .push(format!("{}：{e}", diagnostic_path(root, dir)));
            return;
        }
    };
    // Bound enumeration itself, not merely the retained vector. Sort the bounded
    // batch; an over-limit directory is explicitly partial (read_dir has no sorted API).
    let remaining = MAX_ENTRIES.saturating_sub(*entries);
    let mut paths = BTreeSet::new();
    let mut count = 0usize;
    let mut truncated = false;
    for entry in listing {
        if count >= remaining {
            truncated = true;
            break;
        }
        count += 1;
        match entry {
            Ok(entry) => {
                paths.insert(entry.path());
            }
            Err(e) => result
                .diagnostics
                .push(format!("{}：{e}", diagnostic_path(root, dir))),
        }
    }
    if truncated {
        result.diagnostics.push(format!(
            "{}：目录项已截断（上限 4096；结果仅含有界枚举批次）",
            diagnostic_path(root, dir)
        ));
    }
    *entries += count;
    for path in paths {
        if path
            .file_name()
            .is_some_and(|n| n.to_string_lossy().starts_with('.'))
        {
            continue;
        }
        let meta = match fs::symlink_metadata(&path) {
            Ok(m) => m,
            Err(e) => {
                result
                    .diagnostics
                    .push(format!("{}：{e}", diagnostic_path(root, &path)));
                continue;
            }
        };
        if meta.file_type().is_symlink() {
            result
                .diagnostics
                .push(format!("{}：不读取 symlink", diagnostic_path(root, &path)));
            continue;
        }
        if meta.is_dir() {
            scan(
                root,
                base,
                &path,
                source,
                rules,
                gate,
                depth + 1,
                entries,
                candidates,
                result,
            );
            continue;
        }
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        if !meta.is_file() || !(ext == "md" || rules && ext == "mdc") {
            continue;
        }
        if *candidates >= MAX_FILES {
            result.diagnostics.push(format!(
                "{}：候选文件已截断（上限 256）",
                diagnostic_path(root, &path)
            ));
            break;
        }
        *candidates += 1;
        let parsed = (|| {
            safe_path(root, &path)?;
            readable(gate, &path)?;
            if meta.len() > MAX_FILE_BYTES as u64 {
                return Err("文件超过 64 KiB".to_string());
            }
            let mut bytes = Vec::new();
            File::open(&path)
                .map_err(|e| e.to_string())?
                .take((MAX_HEADER_BYTES + 1) as u64)
                .read_to_end(&mut bytes)
                .map_err(|e| e.to_string())?;
            // A UTF-8 character can straddle the prefix boundary; malformed interior bytes cannot.
            let text = match std::str::from_utf8(&bytes) {
                Ok(t) => t,
                Err(e) if e.error_len().is_none() && meta.len() > bytes.len() as u64 => {
                    std::str::from_utf8(&bytes[..e.valid_up_to()]).map_err(|_| "文件不是 UTF-8")?
                }
                Err(_) => return Err("文件不是 UTF-8".into()),
            };
            let (description, always, body) = parse_document(text).map_err(|e| {
                if text.starts_with("---") && bytes.len() > MAX_HEADER_BYTES {
                    "文件头过大".into()
                } else {
                    e
                }
            })?;
            if body.trim().is_empty() && meta.len() <= bytes.len() as u64 {
                return Err("正文为空".into());
            }
            Ok((description, always))
        })();
        match parsed {
            Ok((description, always_apply)) => {
                let relative = path
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                let name = path
                    .strip_prefix(base)
                    .unwrap()
                    .with_extension("")
                    .to_string_lossy()
                    .replace('\\', "/");
                result.files.push(PromptFile {
                    card: InstructionCard {
                        id: format!("command:{relative}"),
                        kind: InstructionKind::Command,
                        name,
                        description,
                        source: source.to_string(),
                        path: relative,
                    },
                    file_path: path,
                    always_apply,
                });
            }
            Err(e) => result
                .diagnostics
                .push(format!("{}：{e}", diagnostic_path(root, &path))),
        }
    }
}

pub fn render_rules(
    discovery: &Discovery,
    root: &Path,
    gate: &dyn PermissionGate,
    budget: usize,
) -> (String, Vec<String>, usize) {
    let mut diagnostics = discovery.diagnostics.clone();
    let limit = 32_000.min(budget / 10);
    let mut text = String::new();
    let mut loaded = 0;
    for file in &discovery.files {
        if !file.always_apply {
            diagnostics.push(format!("{}：alwaysApply 不是 true", file.card.path));
            continue;
        }
        match read_body(root, &file.file_path, gate) {
            Ok(body) => {
                let header = if text.is_empty() {
                    "## User Custom Instructions\n\n"
                } else {
                    "\n\n"
                };
                let description = if file.card.description.is_empty() {
                    String::new()
                } else {
                    format!("Description: {}\n", file.card.description)
                };
                let entry = format!(
                    "{header}### Source: {}\n{description}{body}",
                    file.card.path
                );
                if text.chars().count() + entry.chars().count() > limit {
                    diagnostics.push(format!("{}：超出章节预算", file.card.path));
                    continue;
                }
                text.push_str(&entry);
                loaded += 1;
            }
            Err(e) => diagnostics.push(format!("{}：{e}", file.card.path)),
        }
    }
    (text, diagnostics, loaded)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::permission::{
        DefaultPermissionGate, GateConfig, PathRule, PathRuleMode, SessionGrants,
    };
    fn gate() -> DefaultPermissionGate {
        DefaultPermissionGate::new(
            GateConfig {
                agent_definition_dir: PathBuf::from("/unused"),
                workspace_roots: vec![],
                agent_trail_readonly_dirs: vec![],
                user_path_rules: vec![],
                user_bash_forbidden: vec![],
                user_bash_approval: vec![],
                auto_confirm: false,
            },
            SessionGrants::new(),
        )
    }
    fn put(root: &Path, path: &str, text: &str) {
        let path = root.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }
    #[test]
    fn metadata_strict_boolean_and_crlf() {
        assert_eq!(
            parse_document("---\r\ndescription: a\r\nalwaysApply: true\r\n---\r\nbody").unwrap(),
            ("a".into(), true, "body")
        );
        for text in [
            "body",
            "---\nalwaysApply: false\n---\nbody",
            "---\nalwaysApply: \"true\"\n---\nbody",
        ] {
            assert!(!parse_document(text).unwrap().1);
        }
        assert!(parse_document("---\ndescription: [\n---\nbody").is_err());
    }
    #[test]
    fn discovers_all_sources_and_duplicates_without_granting_cwd() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let gate = gate();
        assert!(matches!(
            gate.check(PrimitiveOperation::Read, root.to_str().unwrap())
                .unwrap(),
            PermissionDecision::NeedConfirm { .. }
        ));
        for path in [
            ".cursor/commands/git/review.md",
            ".agents/commands/review.md",
            ".data/commands/review.md",
        ] {
            put(
                &root,
                path,
                "---\nname: ignored\ndescription: Review\n---\nbody",
            );
        }
        put(&root, ".cursor/commands/.drafts/x.md", "hidden");
        let d = discover(&root, ".data", false, &gate);
        assert_eq!(d.files.len(), 3);
        assert_eq!(d.files[0].card.name, "git/review");
        assert_eq!(d.files[0].card.id, "command:.cursor/commands/git/review.md");
        gate.grant_path_rule(PathRule {
            path: root.join(".agents").to_string_lossy().into_owned(),
            mode: PathRuleMode::Deny,
        });
        assert_eq!(discover(&root, ".data", false, &gate).files.len(), 2);
    }
    #[test]
    fn rules_extensions_refresh_budget_and_deny() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let gate = gate();
        for ext in ["md", "mdc"] {
            put(
                &root,
                &format!(".cursor/rules/a.{ext}"),
                "---\nalwaysApply: true\ndescription: hello\n---\nrule body",
            );
        }
        put(&root, ".agents/rules/off.md", "body");
        let d = discover(&root, ".agents", true, &gate);
        let (text, errors, count) = render_rules(&d, &root, &gate, 10000);
        assert_eq!(count, 2);
        assert!(text.contains("Description: hello"));
        assert!(errors.iter().any(|e| e.contains("alwaysApply")));
        assert_eq!(render_rules(&d, &root, &gate, 1).2, 0);
        gate.grant_path_rule(PathRule {
            path: root.join(".cursor").to_string_lossy().into_owned(),
            mode: PathRuleMode::Deny,
        });
        assert_eq!(render_rules(&d, &root, &gate, 10000).2, 0);
    }
    #[test]
    fn byte_limit_empty_and_invalid_utf8() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let gate = gate();
        put(&root, ".agents/commands/a.md", &"a".repeat(MAX_FILE_BYTES));
        let path = root.join(".agents/commands/a.md");
        assert_eq!(
            read_body(&root, &path, &gate).unwrap().len(),
            MAX_FILE_BYTES
        );
        fs::write(&path, "a".repeat(MAX_FILE_BYTES + 1)).unwrap();
        assert!(read_body(&root, &path, &gate).is_err());
        fs::write(&path, [255]).unwrap();
        assert!(read_body(&root, &path, &gate).is_err());
        fs::write(&path, " ").unwrap();
        assert!(read_body(&root, &path, &gate).is_err());
    }
    #[cfg(unix)]
    #[test]
    fn rejects_leaf_and_ancestor_links() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let gate = gate();
        put(&root, "real/commands/x.md", "body");
        std::os::unix::fs::symlink(root.join("real"), root.join(".cursor")).unwrap();
        assert!(discover(&root, ".agents", false, &gate).files.is_empty());
        fs::create_dir_all(root.join(".agents/commands")).unwrap();
        std::os::unix::fs::symlink(
            root.join("real/commands/x.md"),
            root.join(".agents/commands/x.md"),
        )
        .unwrap();
        assert!(discover(&root, ".agents", false, &gate).files.is_empty());
    }
    #[test]
    fn deny_is_checked_before_invalid_file_content_and_metadata_are_read() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let gate = gate();
        let path = root.join(".cursor/commands/secret.md");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, [255]).unwrap();
        gate.grant_path_rule(PathRule {
            path: path.to_string_lossy().into_owned(),
            mode: PathRuleMode::Deny,
        });
        let d = discover(&root, ".agents", false, &gate);
        assert!(d.files.is_empty());
        assert!(d.diagnostics.iter().any(|e| e.contains("Deny")));
        assert!(!d.diagnostics.iter().any(|e| e.contains("UTF-8")));
    }

    #[test]
    fn sorted_candidate_truncation() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        for n in (0..257).rev() {
            put(&root, &format!(".cursor/commands/{n:03}.md"), "body");
        }
        let d = discover(&root, ".agents", false, &gate());
        assert_eq!(d.files.len(), 256);
        assert_eq!(d.files[255].card.name, "255");
        assert!(d.diagnostics.iter().any(|e| e.contains("截断")));
    }
}
