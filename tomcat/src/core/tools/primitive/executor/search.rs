//! # `search_files` Tier1（rg/fd）+ Tier2（rust-fallback）
//!
//! 单工具双实现，遵守同一 schema：
//! - **Tier1**：spawn 系统 `rg` / `fd`，最快路径；
//! - **Tier2**：缺二进制时自动回落到 `ignore::WalkBuilder` + `globset` + `regex`，
//!   默认遵守 `.gitignore`；带墙钟、单文件 5 MiB 上限、二进制嗅探与 deny 剪枝。
//!
//! 详见 `docs/architecture/tools/search_files.md` 与 plan §5–§7。

use super::helpers::{find_binary, grant_trigger_str, grant_type_str, permission_scope_str};
use super::DefaultPrimitiveExecutor;
use crate::core::permission::{PathRule, PathRuleMode};
use crate::core::tools::primitive::{
    PrimitiveOperation, SearchFileCount, SearchFileMatch, SearchFilesArgs, SearchFilesOutput,
    SearchFilesOutputMode, SearchFilesQuery, SearchFilesResultMode, SearchFilesStats,
    SearchFilesTarget,
};
use crate::infra::audit::{AuditPrimitiveOp, PrimitiveAuditEntry};
use crate::infra::error::AppError;
use globset::{Glob, GlobSet, GlobSetBuilder};
use ignore::{DirEntry as IgnoreEntry, WalkBuilder};
use regex::RegexBuilder;
use std::io::{BufRead, Read};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::process::Command;

const SEARCH_CONTENT_TIMEOUT_SECS: u64 = 5;
const SEARCH_FILES_TIMEOUT_SECS: u64 = 60;
/// Tier2 单查询墙钟（plan §7.3 冻结值：10s）。
const SEARCH_FALLBACK_TIMEOUT_SECS: u64 = 10;
const SEARCH_CONTENT_DEFAULT_LIMIT: usize = 64;
const SEARCH_FILES_DEFAULT_LIMIT: usize = 128;
const SEARCH_LIMIT_HARD_CAP: usize = 1024;
/// Tier2 单文件大小阈值（plan §7.3 冻结值：5 MiB），超过则跳过并 warning。
const SEARCH_FALLBACK_MAX_FILE_BYTES: u64 = 5 * 1024 * 1024;
/// Tier2 二进制嗅探读取的字节数；命中 NUL 即判定为二进制文件并跳过（T9）。
const SEARCH_FALLBACK_BINARY_SNIFF_BYTES: usize = 8 * 1024;
/// 环境变量：覆盖 Tier2 单查询墙钟（毫秒），便于 CI/性能调优（plan §5.6）。
const SEARCH_FALLBACK_DEADLINE_ENV: &str = "PI_SEARCH_TIER2_DEADLINE_MS";

fn resolve_search_limit(args: &SearchFilesArgs) -> Result<Option<usize>, AppError> {
    let limit = match args.head_limit {
        None => Some(match args.target {
            SearchFilesTarget::Content => SEARCH_CONTENT_DEFAULT_LIMIT,
            SearchFilesTarget::Files => SEARCH_FILES_DEFAULT_LIMIT,
        }),
        Some(None) => None,
        Some(Some(0)) => {
            return Err(AppError::Primitive(
                "search_files.head_limit must be 1..=1024 or null; 0 is not accepted".to_string(),
            ))
        }
        Some(Some(n)) if n > SEARCH_LIMIT_HARD_CAP => {
            return Err(AppError::Primitive(format!(
                "search_files.head_limit must be <= {}",
                SEARCH_LIMIT_HARD_CAP
            )))
        }
        Some(Some(n)) => Some(n),
    };
    Ok(limit)
}

fn search_root_and_arg(path: &Path) -> (PathBuf, String) {
    if path.is_dir() {
        return (path.to_path_buf(), ".".to_string());
    }
    let root = path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let arg = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned());
    (root, arg)
}

fn parse_rg_match_line(line: &str) -> Option<SearchFileMatch> {
    let mut parts = line.splitn(4, ':');
    let path = parts.next()?.to_string();
    let line_no = parts.next()?.parse::<u64>().ok()?;
    let _column = parts.next()?;
    let text = parts.next().unwrap_or("").to_string();
    Some(SearchFileMatch {
        path,
        line: line_no,
        text,
        before: Vec::new(),
        after: Vec::new(),
    })
}

fn parse_rg_count_line(line: &str) -> Option<SearchFileCount> {
    let (path, count) = line.rsplit_once(':')?;
    Some(SearchFileCount {
        path: path.to_string(),
        count: count.parse::<u64>().ok()?,
    })
}

fn paginate<T>(
    items: Vec<T>,
    offset: usize,
    limit: Option<usize>,
) -> (Vec<T>, bool, Option<usize>) {
    let total = items.len();
    let start = offset.min(total);
    let end = match limit {
        Some(limit) => (start + limit).min(total),
        None => total,
    };
    let truncated = end < total;
    let page = items.into_iter().skip(start).take(end - start).collect();
    (page, truncated, truncated.then_some(end))
}

fn absolute_result_path(root: &Path, path: &str) -> PathBuf {
    let p = PathBuf::from(path);
    if p.is_absolute() {
        p
    } else {
        root.join(p)
    }
}

fn filter_denied_files(
    root: &Path,
    files: Vec<String>,
    deny_rules: &[PathRule],
) -> (Vec<String>, usize) {
    let mut skipped = 0;
    let kept = files
        .into_iter()
        .filter(|path| {
            let denied = deny_rules
                .iter()
                .any(|rule| rule.matches(&absolute_result_path(root, path)));
            if denied {
                skipped += 1;
            }
            !denied
        })
        .collect();
    (kept, skipped)
}

fn filter_denied_matches(
    root: &Path,
    matches: Vec<SearchFileMatch>,
    deny_rules: &[PathRule],
) -> (Vec<SearchFileMatch>, usize) {
    let mut skipped = 0;
    let kept = matches
        .into_iter()
        .filter(|item| {
            let denied = deny_rules
                .iter()
                .any(|rule| rule.matches(&absolute_result_path(root, &item.path)));
            if denied {
                skipped += 1;
            }
            !denied
        })
        .collect();
    (kept, skipped)
}

fn filter_denied_counts(
    root: &Path,
    counts: Vec<SearchFileCount>,
    deny_rules: &[PathRule],
) -> (Vec<SearchFileCount>, usize) {
    let mut skipped = 0;
    let kept = counts
        .into_iter()
        .filter(|item| {
            let denied = deny_rules
                .iter()
                .any(|rule| rule.matches(&absolute_result_path(root, &item.path)));
            if denied {
                skipped += 1;
            }
            !denied
        })
        .collect();
    (kept, skipped)
}

fn search_files_query(
    args: &SearchFilesArgs,
    path: &Path,
    limit: Option<usize>,
    output_mode: Option<SearchFilesOutputMode>,
) -> SearchFilesQuery {
    SearchFilesQuery {
        pattern: args.pattern.clone(),
        target: args.target,
        path: path.to_string_lossy().into_owned(),
        glob: if args.target == SearchFilesTarget::Files {
            None
        } else {
            args.glob.clone()
        },
        file_type: if args.target == SearchFilesTarget::Files {
            None
        } else {
            args.file_type.clone()
        },
        output_mode,
        head_limit: limit,
        offset: args.offset,
        case_insensitive: if args.target == SearchFilesTarget::Files {
            false
        } else {
            args.case_insensitive
        },
        include_hidden: args.include_hidden,
    }
}

fn fallback_warning(warnings: &mut Vec<String>) {
    warnings.push(
        "implementation=tier2 rust-fallback; regex dialect is Rust regex and may differ from ripgrep; .gitignore/.ignore are respected by default"
            .to_string(),
    );
}

fn tier1_warning(warnings: &mut Vec<String>) {
    warnings.push("implementation=tier1 rg/fd".to_string());
}

fn normalize_rel_path(path: &Path) -> String {
    path.to_string_lossy()
        .replace(std::path::MAIN_SEPARATOR, "/")
}

fn build_globset(pattern: &str) -> Result<GlobSet, AppError> {
    let mut builder = GlobSetBuilder::new();
    builder.add(Glob::new(pattern).map_err(|e| AppError::Primitive(e.to_string()))?);
    if !pattern.contains('/') && !pattern.contains('\\') {
        let recursive = format!("**/{}", pattern);
        builder.add(Glob::new(&recursive).map_err(|e| AppError::Primitive(e.to_string()))?);
    }
    builder
        .build()
        .map_err(|e| AppError::Primitive(e.to_string()))
}

fn file_type_extension(file_type: &str) -> Option<&'static str> {
    match file_type.to_ascii_lowercase().as_str() {
        "rust" | "rs" => Some("rs"),
        "javascript" | "js" => Some("js"),
        "typescript" | "ts" => Some("ts"),
        "python" | "py" => Some("py"),
        "markdown" | "md" => Some("md"),
        "toml" => Some("toml"),
        "json" => Some("json"),
        "yaml" | "yml" => Some("yml"),
        _ => None,
    }
}

fn matches_file_type(path: &Path, file_type: Option<&str>, warnings: &mut Vec<String>) -> bool {
    let Some(file_type) = file_type else {
        return true;
    };
    let Some(expected) = file_type_extension(file_type) else {
        warnings.push(format!(
            "tier2 ignored unsupported file_type={}; filtering by type was not applied",
            file_type
        ));
        return true;
    };
    let Some(ext) = path.extension().and_then(|ext| ext.to_str()) else {
        return false;
    };
    if expected == "yml" {
        ext.eq_ignore_ascii_case("yml") || ext.eq_ignore_ascii_case("yaml")
    } else {
        ext.eq_ignore_ascii_case(expected)
    }
}

/// 计算 Tier2 单查询墙钟；`PI_SEARCH_TIER2_DEADLINE_MS` 可覆盖默认 10s。
fn fallback_deadline() -> Duration {
    if let Some(ms) = std::env::var(SEARCH_FALLBACK_DEADLINE_ENV)
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
    {
        return Duration::from_millis(ms);
    }
    Duration::from_secs(SEARCH_FALLBACK_TIMEOUT_SECS)
}

/// 是否触达 Tier2 墙钟；命中后调用方应 `truncated=true` 并写 warning，不返回错误。
fn fallback_deadline_hit(started: Instant, deadline: Duration) -> bool {
    started.elapsed() >= deadline
}

fn fallback_timeout_warning(warnings: &mut Vec<String>, deadline: Duration) {
    warnings.push(format!(
        "tier2 wall-clock budget {}ms exhausted; result truncated. Override with {}=<ms> or narrow path/glob.",
        deadline.as_millis(),
        SEARCH_FALLBACK_DEADLINE_ENV
    ));
}

/// 嗅探文件前若干字节，命中 NUL 即视为二进制文件，配合大文件阈值过滤掉媒体/可执行文件。
fn is_binary_file(path: &Path) -> bool {
    let Ok(mut file) = std::fs::File::open(path) else {
        return false;
    };
    let mut buf = [0u8; SEARCH_FALLBACK_BINARY_SNIFF_BYTES];
    match file.read(&mut buf) {
        Ok(n) => buf[..n].contains(&0),
        Err(_) => false,
    }
}

/// 用 `ignore::WalkBuilder` 列举授权根下的候选文件。
///
/// - 默认遵守 `.gitignore`/`.ignore`/`.git/info/exclude`。
/// - `filter_entry` 阶段对 deny 路径剪枝：拒绝目录直接不递归，避免越权 IO。
/// - 大文件 / 二进制文件直接跳过并写 warning（T9）。
fn collect_fallback_files(
    root: &Path,
    path: &Path,
    args: &SearchFilesArgs,
    deny_rules: &[PathRule],
    warnings: &mut Vec<String>,
) -> Result<Vec<(String, PathBuf)>, AppError> {
    let globset = match args.target {
        SearchFilesTarget::Files => Some(build_globset(&args.pattern)?),
        SearchFilesTarget::Content => args.glob.as_deref().map(build_globset).transpose()?,
    };
    let start = if path.is_file() { path } else { root };

    let mut builder = WalkBuilder::new(start);
    builder
        .standard_filters(true)
        .hidden(!args.include_hidden)
        .follow_links(false);
    let deny_clone = deny_rules.to_vec();
    let pruned = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let pruned_for_filter = Arc::clone(&pruned);
    builder.filter_entry(move |entry: &IgnoreEntry| {
        if deny_clone.iter().any(|rule| rule.matches(entry.path())) {
            pruned_for_filter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            return false;
        }
        true
    });

    let mut files = Vec::new();
    let mut skipped_large = 0usize;
    let mut skipped_binary = 0usize;
    for result in builder.build() {
        let entry = match result {
            Ok(entry) => entry,
            Err(_) => continue,
        };
        let Some(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_file() {
            continue;
        }
        let abs = entry.path().to_path_buf();
        let extension_filter = (args.target == SearchFilesTarget::Content)
            .then_some(args.file_type.as_deref())
            .flatten();
        if !matches_file_type(&abs, extension_filter, warnings) {
            continue;
        }
        if let Ok(meta) = std::fs::metadata(&abs) {
            if meta.len() > SEARCH_FALLBACK_MAX_FILE_BYTES {
                skipped_large += 1;
                continue;
            }
        }
        if args.target == SearchFilesTarget::Content && is_binary_file(&abs) {
            skipped_binary += 1;
            continue;
        }
        let rel = abs.strip_prefix(root).unwrap_or(&abs).to_path_buf();
        let rel_str = normalize_rel_path(&rel);
        if globset
            .as_ref()
            .is_some_and(|globset| !globset.is_match(&rel_str))
        {
            continue;
        }
        files.push((rel_str, abs));
    }
    if skipped_large > 0 {
        warnings.push(format!(
            "tier2 skipped {} files larger than {} bytes",
            skipped_large, SEARCH_FALLBACK_MAX_FILE_BYTES
        ));
    }
    if skipped_binary > 0 {
        warnings.push(format!(
            "tier2 skipped {} binary files (NUL byte detected in first {} bytes)",
            skipped_binary, SEARCH_FALLBACK_BINARY_SNIFF_BYTES
        ));
    }
    let pruned_count = pruned.load(std::sync::atomic::Ordering::Relaxed);
    if pruned_count > 0 {
        warnings.push(format!(
            "skipped {} paths due to read deny (tier2 pruned at filter_entry)",
            pruned_count
        ));
    }
    files.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(files)
}

/// Tier2 主入口：在阻塞线程上执行（由调用方包裹 `spawn_blocking`）。
///
/// 关键约束：
/// - 墙钟超时：截断 + warning，**不返回 Err**（plan §5.6）。
/// - regex 编译失败：返回空命中集 + warning，**不 panic / 不 Err**（T8 lookaround / back-ref）。
/// - deny 已在 `collect_fallback_files` 的 `filter_entry` 阶段剪枝；此处是叶子复检，避免越权 IO。
fn search_files_fallback(
    args: SearchFilesArgs,
    root: PathBuf,
    path: PathBuf,
    limit: Option<usize>,
    deny_rules: Vec<PathRule>,
    started: Instant,
) -> Result<SearchFilesOutput, AppError> {
    let mut warnings = Vec::new();
    fallback_warning(&mut warnings);
    let deadline = fallback_deadline();
    let candidates = collect_fallback_files(&root, &path, &args, &deny_rules, &mut warnings)?;

    match args.target {
        SearchFilesTarget::Files => {
            let files = candidates
                .into_iter()
                .map(|(rel, _)| rel)
                .collect::<Vec<_>>();
            let scanned = files.len();
            let (files, skipped) = filter_denied_files(&root, files, &deny_rules);
            if skipped > 0 {
                warnings.push(format!("skipped {} paths due to read deny", skipped));
            }
            let (files, mut truncated, next_offset) = paginate(files, args.offset, limit);
            if fallback_deadline_hit(started, deadline) {
                truncated = true;
                fallback_timeout_warning(&mut warnings, deadline);
            }
            Ok(SearchFilesOutput {
                mode: SearchFilesResultMode::Files,
                query: search_files_query(&args, &path, limit, None),
                files: Some(files),
                matches: None,
                counts: None,
                stats: SearchFilesStats {
                    scanned_files: scanned,
                    elapsed_ms: started.elapsed().as_millis(),
                },
                truncated,
                next_offset,
                warnings,
            })
        }
        SearchFilesTarget::Content => {
            let regex = match RegexBuilder::new(&args.pattern)
                .case_insensitive(args.case_insensitive)
                .build()
            {
                Ok(re) => Some(re),
                Err(e) => {
                    warnings.push(format!(
                        "tier2 unsupported regex (likely lookaround/back-reference): {}; returning empty match set",
                        e
                    ));
                    None
                }
            };
            if args.context.is_some_and(|context| context > 0)
                && args.output_mode == SearchFilesOutputMode::Content
            {
                warnings.push(
                    "tier2 does not currently include before/after context lines".to_string(),
                );
            }
            let scanned = candidates.len();
            let mut deadline_tripped = false;
            match args.output_mode {
                SearchFilesOutputMode::FilesWithMatches => {
                    let mut files = Vec::new();
                    if let Some(regex) = regex.as_ref() {
                        for (rel, abs) in candidates {
                            if file_has_match(&abs, regex, &mut warnings)? {
                                files.push(rel);
                            }
                            if fallback_deadline_hit(started, deadline) {
                                deadline_tripped = true;
                                break;
                            }
                        }
                    }
                    let (files, mut truncated, next_offset) = paginate(files, args.offset, limit);
                    if deadline_tripped {
                        truncated = true;
                        fallback_timeout_warning(&mut warnings, deadline);
                    }
                    Ok(SearchFilesOutput {
                        mode: SearchFilesResultMode::ContentFiles,
                        query: search_files_query(&args, &path, limit, Some(args.output_mode)),
                        files: Some(files),
                        matches: None,
                        counts: None,
                        stats: SearchFilesStats {
                            scanned_files: scanned,
                            elapsed_ms: started.elapsed().as_millis(),
                        },
                        truncated,
                        next_offset,
                        warnings,
                    })
                }
                SearchFilesOutputMode::Count => {
                    let mut counts = Vec::new();
                    if let Some(regex) = regex.as_ref() {
                        for (rel, abs) in candidates {
                            let count = file_match_count(&abs, regex, &mut warnings)?;
                            if count > 0 {
                                counts.push(SearchFileCount { path: rel, count });
                            }
                            if fallback_deadline_hit(started, deadline) {
                                deadline_tripped = true;
                                break;
                            }
                        }
                    }
                    let (counts, mut truncated, next_offset) = paginate(counts, args.offset, limit);
                    if deadline_tripped {
                        truncated = true;
                        fallback_timeout_warning(&mut warnings, deadline);
                    }
                    Ok(SearchFilesOutput {
                        mode: SearchFilesResultMode::ContentCount,
                        query: search_files_query(&args, &path, limit, Some(args.output_mode)),
                        files: None,
                        matches: None,
                        counts: Some(counts),
                        stats: SearchFilesStats {
                            scanned_files: scanned,
                            elapsed_ms: started.elapsed().as_millis(),
                        },
                        truncated,
                        next_offset,
                        warnings,
                    })
                }
                SearchFilesOutputMode::Content => {
                    let mut matches = Vec::new();
                    if let Some(regex) = regex.as_ref() {
                        for (rel, abs) in candidates {
                            collect_file_matches(&rel, &abs, regex, &mut matches, &mut warnings)?;
                            if fallback_deadline_hit(started, deadline) {
                                deadline_tripped = true;
                                break;
                            }
                        }
                    }
                    let (matches, mut truncated, next_offset) =
                        paginate(matches, args.offset, limit);
                    if deadline_tripped {
                        truncated = true;
                        fallback_timeout_warning(&mut warnings, deadline);
                    }
                    Ok(SearchFilesOutput {
                        mode: SearchFilesResultMode::ContentLines,
                        query: search_files_query(&args, &path, limit, Some(args.output_mode)),
                        files: None,
                        matches: Some(matches),
                        counts: None,
                        stats: SearchFilesStats {
                            scanned_files: scanned,
                            elapsed_ms: started.elapsed().as_millis(),
                        },
                        truncated,
                        next_offset,
                        warnings,
                    })
                }
            }
        }
    }
}

fn file_has_match(
    path: &Path,
    regex: &regex::Regex,
    warnings: &mut Vec<String>,
) -> Result<bool, AppError> {
    Ok(file_match_count(path, regex, warnings)? > 0)
}

fn file_match_count(
    path: &Path,
    regex: &regex::Regex,
    warnings: &mut Vec<String>,
) -> Result<u64, AppError> {
    let mut count = 0;
    visit_text_lines(path, warnings, |_, line| {
        if regex.is_match(line) {
            count += 1;
        }
    })?;
    Ok(count)
}

fn collect_file_matches(
    rel: &str,
    path: &Path,
    regex: &regex::Regex,
    matches: &mut Vec<SearchFileMatch>,
    warnings: &mut Vec<String>,
) -> Result<(), AppError> {
    visit_text_lines(path, warnings, |line_no, line| {
        if regex.is_match(line) {
            matches.push(SearchFileMatch {
                path: rel.to_string(),
                line: line_no,
                text: line.trim_end_matches(['\r', '\n']).to_string(),
                before: Vec::new(),
                after: Vec::new(),
            });
        }
    })
}

fn visit_text_lines<F>(
    path: &Path,
    warnings: &mut Vec<String>,
    mut visitor: F,
) -> Result<(), AppError>
where
    F: FnMut(u64, &str),
{
    let meta = std::fs::metadata(path).map_err(AppError::Io)?;
    if meta.len() > SEARCH_FALLBACK_MAX_FILE_BYTES {
        warnings.push(format!(
            "tier2 skipped large file over {} bytes: {}",
            SEARCH_FALLBACK_MAX_FILE_BYTES,
            path.display()
        ));
        return Ok(());
    }
    let file = std::fs::File::open(path).map_err(AppError::Io)?;
    let mut reader = std::io::BufReader::new(file);
    let mut buf = Vec::new();
    let mut line_no = 1u64;
    loop {
        buf.clear();
        let n = reader.read_until(b'\n', &mut buf).map_err(AppError::Io)?;
        if n == 0 {
            break;
        }
        let line = String::from_utf8_lossy(&buf);
        visitor(line_no, &line);
        line_no += 1;
    }
    Ok(())
}

pub(super) async fn search_files_impl(
    executor: &DefaultPrimitiveExecutor,
    args: SearchFilesArgs,
    plugin_id: &str,
) -> Result<SearchFilesOutput, AppError> {
    if args.pattern.trim().is_empty() {
        return Err(AppError::Primitive(
            "search_files.pattern is required".to_string(),
        ));
    }

    let started = Instant::now();
    let requested_path = args.path.clone().unwrap_or_else(|| ".".to_string());
    if let Some(err) = super::helpers::url_like_fs_miss(&requested_path) {
        return Err(err);
    }
    let (path_buf, scope, grant) = executor
        .gate_check_path(PrimitiveOperation::Read, &requested_path, plugin_id)
        .await?;
    let (root, search_arg) = search_root_and_arg(&path_buf);
    let limit = resolve_search_limit(&args)?;
    let deny_rules: Vec<PathRule> = executor
        .gate
        .effective_path_rules()
        .into_iter()
        .filter(|rule| rule.mode == PathRuleMode::Deny)
        .collect();
    let mut warnings = Vec::new();

    let output = match args.target {
        SearchFilesTarget::Files => {
            if let Some(fd) = find_binary(&["fd", "fdfind"]) {
                tier1_warning(&mut warnings);
                let mut cmd = Command::new(fd);
                cmd.arg("--color=never")
                    .arg("--type")
                    .arg("f")
                    .arg("--glob")
                    .arg(&args.pattern);
                if args.include_hidden {
                    cmd.arg("--hidden");
                }
                cmd.arg(&search_arg).current_dir(&root).kill_on_drop(true);
                let output = executor
                    .run_search_command(cmd, SEARCH_FILES_TIMEOUT_SECS)
                    .await?;
                if !output.status.success() {
                    return Err(AppError::Primitive(
                        String::from_utf8_lossy(&output.stderr).trim().to_string(),
                    ));
                }
                let files = String::from_utf8_lossy(&output.stdout)
                    .lines()
                    .filter(|line| !line.trim().is_empty())
                    .map(str::to_string)
                    .collect::<Vec<_>>();
                let (files, skipped) = filter_denied_files(&root, files, &deny_rules);
                if skipped > 0 {
                    warnings.push(format!("skipped {} paths due to read deny", skipped));
                }
                let scanned = files.len();
                let (files, truncated, next_offset) = paginate(files, args.offset, limit);
                SearchFilesOutput {
                    mode: SearchFilesResultMode::Files,
                    query: search_files_query(&args, &path_buf, limit, None),
                    files: Some(files),
                    matches: None,
                    counts: None,
                    stats: SearchFilesStats {
                        scanned_files: scanned,
                        elapsed_ms: started.elapsed().as_millis(),
                    },
                    truncated,
                    next_offset,
                    warnings,
                }
            } else {
                let args = args.clone();
                let root = root.clone();
                let path_buf = path_buf.clone();
                let deny_rules = deny_rules.clone();
                tokio::task::spawn_blocking(move || {
                    search_files_fallback(args, root, path_buf, limit, deny_rules, started)
                })
                .await
                .map_err(|e| AppError::Primitive(e.to_string()))??
            }
        }
        SearchFilesTarget::Content => {
            if let Some(rg) = find_binary(&["rg", "ripgrep"]) {
                tier1_warning(&mut warnings);
                let mut cmd = Command::new(rg);
                cmd.arg("--color=never");
                match args.output_mode {
                    SearchFilesOutputMode::FilesWithMatches => {
                        cmd.arg("--files-with-matches");
                    }
                    SearchFilesOutputMode::Count => {
                        cmd.arg("--count");
                    }
                    SearchFilesOutputMode::Content => {
                        cmd.arg("--line-number")
                            .arg("--column")
                            .arg("--with-filename")
                            .arg("--no-heading")
                            .arg("--max-columns")
                            .arg("500");
                        if let Some(context) = args.context.filter(|context| *context > 0) {
                            cmd.arg("-C").arg(context.to_string());
                        }
                    }
                }
                if args.case_insensitive {
                    cmd.arg("-i");
                }
                if args.include_hidden {
                    cmd.arg("--hidden");
                }
                if let Some(glob) = args.glob.as_deref() {
                    cmd.arg("--glob").arg(glob);
                }
                if let Some(file_type) = args.file_type.as_deref() {
                    cmd.arg("--type").arg(file_type);
                }
                cmd.arg(&args.pattern)
                    .arg(&search_arg)
                    .current_dir(&root)
                    .kill_on_drop(true);
                let output = executor
                    .run_search_command(cmd, SEARCH_CONTENT_TIMEOUT_SECS)
                    .await?;
                let exit = output.status.code().unwrap_or(-1);
                if exit > 1 {
                    return Err(AppError::Primitive(
                        String::from_utf8_lossy(&output.stderr).trim().to_string(),
                    ));
                }
                let stdout = String::from_utf8_lossy(&output.stdout);
                match args.output_mode {
                    SearchFilesOutputMode::FilesWithMatches => {
                        let files = stdout
                            .lines()
                            .filter(|line| !line.trim().is_empty())
                            .map(str::to_string)
                            .collect::<Vec<_>>();
                        let (files, skipped) = filter_denied_files(&root, files, &deny_rules);
                        if skipped > 0 {
                            warnings.push(format!("skipped {} paths due to read deny", skipped));
                        }
                        let scanned = files.len();
                        let (files, truncated, next_offset) = paginate(files, args.offset, limit);
                        SearchFilesOutput {
                            mode: SearchFilesResultMode::ContentFiles,
                            query: SearchFilesQuery {
                                pattern: args.pattern.clone(),
                                target: args.target,
                                path: path_buf.to_string_lossy().into_owned(),
                                glob: args.glob.clone(),
                                file_type: args.file_type.clone(),
                                output_mode: Some(args.output_mode),
                                head_limit: limit,
                                offset: args.offset,
                                case_insensitive: args.case_insensitive,
                                include_hidden: args.include_hidden,
                            },
                            files: Some(files),
                            matches: None,
                            counts: None,
                            stats: SearchFilesStats {
                                scanned_files: scanned,
                                elapsed_ms: started.elapsed().as_millis(),
                            },
                            truncated,
                            next_offset,
                            warnings,
                        }
                    }
                    SearchFilesOutputMode::Count => {
                        let counts = stdout
                            .lines()
                            .filter_map(parse_rg_count_line)
                            .collect::<Vec<_>>();
                        let (counts, skipped) = filter_denied_counts(&root, counts, &deny_rules);
                        if skipped > 0 {
                            warnings.push(format!("skipped {} paths due to read deny", skipped));
                        }
                        let scanned = counts.len();
                        let (counts, truncated, next_offset) = paginate(counts, args.offset, limit);
                        SearchFilesOutput {
                            mode: SearchFilesResultMode::ContentCount,
                            query: SearchFilesQuery {
                                pattern: args.pattern.clone(),
                                target: args.target,
                                path: path_buf.to_string_lossy().into_owned(),
                                glob: args.glob.clone(),
                                file_type: args.file_type.clone(),
                                output_mode: Some(args.output_mode),
                                head_limit: limit,
                                offset: args.offset,
                                case_insensitive: args.case_insensitive,
                                include_hidden: args.include_hidden,
                            },
                            files: None,
                            matches: None,
                            counts: Some(counts),
                            stats: SearchFilesStats {
                                scanned_files: scanned,
                                elapsed_ms: started.elapsed().as_millis(),
                            },
                            truncated,
                            next_offset,
                            warnings,
                        }
                    }
                    SearchFilesOutputMode::Content => {
                        let matches = stdout
                            .lines()
                            .filter_map(parse_rg_match_line)
                            .collect::<Vec<_>>();
                        let (matches, skipped) = filter_denied_matches(&root, matches, &deny_rules);
                        if skipped > 0 {
                            warnings.push(format!("skipped {} paths due to read deny", skipped));
                        }
                        let scanned = matches.len();
                        let (matches, truncated, next_offset) =
                            paginate(matches, args.offset, limit);
                        SearchFilesOutput {
                            mode: SearchFilesResultMode::ContentLines,
                            query: SearchFilesQuery {
                                pattern: args.pattern.clone(),
                                target: args.target,
                                path: path_buf.to_string_lossy().into_owned(),
                                glob: args.glob.clone(),
                                file_type: args.file_type.clone(),
                                output_mode: Some(args.output_mode),
                                head_limit: limit,
                                offset: args.offset,
                                case_insensitive: args.case_insensitive,
                                include_hidden: args.include_hidden,
                            },
                            files: None,
                            matches: Some(matches),
                            counts: None,
                            stats: SearchFilesStats {
                                scanned_files: scanned,
                                elapsed_ms: started.elapsed().as_millis(),
                            },
                            truncated,
                            next_offset,
                            warnings,
                        }
                    }
                }
            } else {
                let args = args.clone();
                let root = root.clone();
                let path_buf = path_buf.clone();
                let deny_rules = deny_rules.clone();
                tokio::task::spawn_blocking(move || {
                    search_files_fallback(args, root, path_buf, limit, deny_rules, started)
                })
                .await
                .map_err(|e| AppError::Primitive(e.to_string()))??
            }
        }
    };

    executor.audit.record_primitive(PrimitiveAuditEntry {
        operation: AuditPrimitiveOp::Read,
        path_or_cmd: format!("search_files {}", requested_path),
        plugin_id: plugin_id.to_string(),
        user_approved: true,
        success: true,
        detail: Some(format!(
            "mode={:?} truncated={}",
            output.mode, output.truncated
        )),
        permission_scope: Some(permission_scope_str(scope)),
        grant_type: Some(grant_type_str(grant.grant_type)),
        grant_trigger: Some(grant_trigger_str(grant.trigger)),
    });

    Ok(output)
}
