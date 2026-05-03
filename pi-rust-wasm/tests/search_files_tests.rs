use pi_wasm::core::permission::{
    DefaultPermissionGate, GateConfig, PathRule, PathRuleMode, PermissionGate, SessionGrants,
};
use pi_wasm::{
    AllowAllConfirmation, DefaultPrimitiveExecutor, PrimitiveConfig, PrimitiveExecutor,
    SearchFilesArgs, SearchFilesOutputMode, SearchFilesTarget, TracingAuditRecorder,
};
use serial_test::serial;
use std::sync::Arc;
use tempfile::TempDir;

struct PathGuard {
    old_path: Option<std::ffi::OsString>,
}

impl PathGuard {
    fn set(path: &std::path::Path) -> Self {
        let old_path = std::env::var_os("PATH");
        std::env::set_var("PATH", path);
        Self { old_path }
    }
}

impl Drop for PathGuard {
    fn drop(&mut self) {
        if let Some(old_path) = self.old_path.take() {
            std::env::set_var("PATH", old_path);
        } else {
            std::env::remove_var("PATH");
        }
    }
}

fn make_gate(
    definition: &std::path::Path,
    user_path_rules: Vec<PathRule>,
) -> Arc<dyn PermissionGate> {
    DefaultPermissionGate::new(
        GateConfig {
            agent_definition_dir: definition.to_path_buf(),
            workspace_roots: vec![],
            agent_trail_readonly_dirs: vec![],
            user_path_rules,
            user_bash_forbidden: vec![],
            user_bash_approval: vec![],
            auto_confirm: true,
        },
        SessionGrants::new(),
    )
    .into_arc()
}

fn make_executor(
    definition: &std::path::Path,
    user_path_rules: Vec<PathRule>,
) -> DefaultPrimitiveExecutor {
    DefaultPrimitiveExecutor::new(
        PrimitiveConfig::default(),
        Arc::new(AllowAllConfirmation),
        Arc::new(TracingAuditRecorder),
        make_gate(definition, user_path_rules),
    )
}

#[cfg(unix)]
fn write_executable(path: &std::path::Path, content: &str) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    std::fs::write(path, content)?;
    let mut perms = std::fs::metadata(path)?.permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(path, perms)
}

#[tokio::test]
#[serial(env_lock)]
async fn test_search_files_content_files_with_matches_paginates_and_filters_denied(
) -> Result<(), Box<dyn std::error::Error>> {
    let tmp = TempDir::new()?;
    let root = tmp.path().join("workspace");
    let bin = tmp.path().join("bin");
    std::fs::create_dir_all(root.join("src"))?;
    std::fs::create_dir_all(root.join("secret"))?;
    std::fs::create_dir_all(&bin)?;
    std::fs::write(root.join("src/lib.rs"), "needle\n")?;
    std::fs::write(root.join("src/main.rs"), "needle\n")?;
    std::fs::write(root.join("secret/token.rs"), "needle\n")?;
    write_executable(
        &bin.join("rg"),
        "#!/bin/sh\nprintf 'src/lib.rs\\nsrc/main.rs\\nsecret/token.rs\\n'\n",
    )?;
    let _path = PathGuard::set(&bin);

    let deny = PathRule::new(root.join("secret").to_string_lossy(), PathRuleMode::Deny);
    let executor = make_executor(&root.canonicalize()?, vec![deny]);
    let out = executor
        .search_files(
            SearchFilesArgs {
                pattern: "needle".to_string(),
                target: SearchFilesTarget::Content,
                path: Some(root.to_string_lossy().into_owned()),
                glob: Some("*.rs".to_string()),
                file_type: None,
                output_mode: SearchFilesOutputMode::FilesWithMatches,
                context: None,
                head_limit: Some(Some(1)),
                offset: 0,
                case_insensitive: false,
                include_hidden: false,
            },
            "test_plugin",
        )
        .await?;

    assert_eq!(out.files.as_ref().expect("files").len(), 1);
    assert_eq!(out.files.as_ref().unwrap()[0], "src/lib.rs");
    assert!(out.truncated);
    assert_eq!(out.next_offset, Some(1));
    assert!(
        out.warnings
            .iter()
            .any(|warning| warning.contains("read deny")),
        "deny 子树应通过 warnings 汇报"
    );
    Ok(())
}

#[tokio::test]
#[serial(env_lock)]
async fn test_search_files_content_lines_and_count_modes() -> Result<(), Box<dyn std::error::Error>>
{
    let tmp = TempDir::new()?;
    let root = tmp.path().join("workspace");
    let bin = tmp.path().join("bin");
    std::fs::create_dir_all(root.join("src"))?;
    std::fs::create_dir_all(&bin)?;
    std::fs::write(root.join("src/lib.rs"), "needle\n")?;
    write_executable(
        &bin.join("rg"),
        r#"#!/bin/sh
for arg in "$@"; do
  if [ "$arg" = "--count" ]; then
    printf 'src/lib.rs:2\n'
    exit 0
  fi
done
printf 'src/lib.rs:3:1:needle here\n'
"#,
    )?;
    let _path = PathGuard::set(&bin);

    let executor = make_executor(&root.canonicalize()?, vec![]);
    let lines = executor
        .search_files(
            SearchFilesArgs {
                pattern: "needle".to_string(),
                target: SearchFilesTarget::Content,
                path: Some(root.to_string_lossy().into_owned()),
                glob: None,
                file_type: None,
                output_mode: SearchFilesOutputMode::Content,
                context: Some(1),
                head_limit: Some(Some(10)),
                offset: 0,
                case_insensitive: true,
                include_hidden: false,
            },
            "test_plugin",
        )
        .await?;
    assert_eq!(lines.matches.as_ref().expect("matches")[0].line, 3);
    assert!(lines.matches.as_ref().unwrap()[0].text.contains("needle"));

    let counts = executor
        .search_files(
            SearchFilesArgs {
                pattern: "needle".to_string(),
                target: SearchFilesTarget::Content,
                path: Some(root.to_string_lossy().into_owned()),
                glob: None,
                file_type: None,
                output_mode: SearchFilesOutputMode::Count,
                context: None,
                head_limit: Some(Some(10)),
                offset: 0,
                case_insensitive: false,
                include_hidden: false,
            },
            "test_plugin",
        )
        .await?;
    assert_eq!(counts.counts.as_ref().expect("counts")[0].count, 2);
    Ok(())
}

#[tokio::test]
#[serial(env_lock)]
async fn test_search_files_target_files_uses_fd_glob() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = TempDir::new()?;
    let root = tmp.path().join("workspace");
    let bin = tmp.path().join("bin");
    std::fs::create_dir_all(root.join("src"))?;
    std::fs::create_dir_all(root.join("secret"))?;
    std::fs::create_dir_all(&bin)?;
    write_executable(
        &bin.join("fd"),
        "#!/bin/sh\nprintf 'src/lib.rs\\nsrc/main.rs\\nsecret/token.rs\\n'\n",
    )?;
    let _path = PathGuard::set(&bin);

    let deny = PathRule::new(root.join("secret").to_string_lossy(), PathRuleMode::Deny);
    let executor = make_executor(&root.canonicalize()?, vec![deny]);
    let out = executor
        .search_files(
            SearchFilesArgs {
                pattern: "**/*.rs".to_string(),
                target: SearchFilesTarget::Files,
                path: Some(root.to_string_lossy().into_owned()),
                glob: Some("ignored".to_string()),
                file_type: Some("rust".to_string()),
                output_mode: SearchFilesOutputMode::Content,
                context: Some(10),
                head_limit: None,
                offset: 0,
                case_insensitive: true,
                include_hidden: false,
            },
            "test_plugin",
        )
        .await?;

    let files = out.files.expect("files");
    assert_eq!(files, vec!["src/lib.rs", "src/main.rs"]);
    assert!(out.query.output_mode.is_none());
    assert!(out.query.glob.is_none());
    assert!(out.warnings.iter().any(|w| w.contains("read deny")));
    Ok(())
}

#[tokio::test]
#[serial(env_lock)]
async fn test_search_files_missing_binary_uses_tier2_content_fallback(
) -> Result<(), Box<dyn std::error::Error>> {
    let tmp = TempDir::new()?;
    let root = tmp.path().join("workspace");
    let bin = tmp.path().join("empty-bin");
    std::fs::create_dir_all(root.join("src"))?;
    std::fs::create_dir_all(&bin)?;
    std::fs::write(root.join("src/lib.rs"), "Needle\nother\nneedle again\n")?;
    let _path = PathGuard::set(&bin);

    let executor = make_executor(&root.canonicalize()?, vec![]);
    let out = executor
        .search_files(
            SearchFilesArgs {
                pattern: "needle".to_string(),
                target: SearchFilesTarget::Content,
                path: Some(root.to_string_lossy().into_owned()),
                glob: None,
                file_type: None,
                output_mode: SearchFilesOutputMode::FilesWithMatches,
                context: None,
                head_limit: None,
                offset: 0,
                case_insensitive: false,
                include_hidden: false,
            },
            "test_plugin",
        )
        .await?;

    assert_eq!(out.files.as_ref().expect("files"), &vec!["src/lib.rs"]);
    assert!(
        out.warnings
            .iter()
            .any(|w| w.contains("implementation=tier2")),
        "missing rg should fall back to the in-process tier2 implementation"
    );
    Ok(())
}

#[tokio::test]
#[serial(env_lock)]
async fn test_search_files_missing_fd_uses_tier2_files_fallback(
) -> Result<(), Box<dyn std::error::Error>> {
    let tmp = TempDir::new()?;
    let root = tmp.path().join("workspace");
    let bin = tmp.path().join("empty-bin");
    std::fs::create_dir_all(root.join("src"))?;
    std::fs::create_dir_all(root.join(".hidden"))?;
    std::fs::create_dir_all(&bin)?;
    std::fs::write(root.join("src/lib.rs"), "needle\n")?;
    std::fs::write(root.join(".hidden/lib.rs"), "needle\n")?;
    let _path = PathGuard::set(&bin);

    let executor = make_executor(&root.canonicalize()?, vec![]);
    let out = executor
        .search_files(
            SearchFilesArgs {
                pattern: "*.rs".to_string(),
                target: SearchFilesTarget::Files,
                path: Some(root.to_string_lossy().into_owned()),
                glob: None,
                file_type: None,
                output_mode: SearchFilesOutputMode::Content,
                context: None,
                head_limit: None,
                offset: 0,
                case_insensitive: false,
                include_hidden: false,
            },
            "test_plugin",
        )
        .await?;

    assert_eq!(out.files.as_ref().expect("files"), &vec!["src/lib.rs"]);
    assert!(out
        .warnings
        .iter()
        .any(|w| w.contains("implementation=tier2")));
    Ok(())
}

#[tokio::test]
#[serial(env_lock)]
async fn test_search_files_tier2_count_and_deny() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = TempDir::new()?;
    let root = tmp.path().join("workspace");
    let bin = tmp.path().join("empty-bin");
    std::fs::create_dir_all(root.join("src"))?;
    std::fs::create_dir_all(root.join("secret"))?;
    std::fs::create_dir_all(&bin)?;
    std::fs::write(root.join("src/lib.rs"), "needle\nneedle\n")?;
    std::fs::write(root.join("secret/token.rs"), "needle\n")?;
    let _path = PathGuard::set(&bin);

    let deny = PathRule::new(root.join("secret").to_string_lossy(), PathRuleMode::Deny);
    let executor = make_executor(&root.canonicalize()?, vec![deny]);
    let out = executor
        .search_files(
            SearchFilesArgs {
                pattern: "needle".to_string(),
                target: SearchFilesTarget::Content,
                path: Some(root.to_string_lossy().into_owned()),
                glob: Some("*.rs".to_string()),
                file_type: Some("rust".to_string()),
                output_mode: SearchFilesOutputMode::Count,
                context: None,
                head_limit: None,
                offset: 0,
                case_insensitive: false,
                include_hidden: false,
            },
            "test_plugin",
        )
        .await?;

    let counts = out.counts.as_ref().expect("counts");
    assert_eq!(counts.len(), 1);
    assert_eq!(counts[0].path, "src/lib.rs");
    assert_eq!(counts[0].count, 2);
    assert!(out.warnings.iter().any(|w| w.contains("read deny")));
    Ok(())
}

/// T8 from plan §3.3: Tier2 must surface a regex compile error as a warning
/// (lookaround / back-reference), return an empty match set, and never panic.
#[tokio::test]
#[serial(env_lock)]
async fn test_search_files_tier2_lookaround_returns_empty_with_warning(
) -> Result<(), Box<dyn std::error::Error>> {
    let tmp = TempDir::new()?;
    let root = tmp.path().join("workspace");
    let bin = tmp.path().join("empty-bin");
    std::fs::create_dir_all(root.join("src"))?;
    std::fs::create_dir_all(&bin)?;
    std::fs::write(root.join("src/lib.rs"), "needle\nhaystack\n")?;
    let _path = PathGuard::set(&bin);

    let executor = make_executor(&root.canonicalize()?, vec![]);
    let out = executor
        .search_files(
            SearchFilesArgs {
                pattern: "(?=needle)".to_string(),
                target: SearchFilesTarget::Content,
                path: Some(root.to_string_lossy().into_owned()),
                glob: None,
                file_type: None,
                output_mode: SearchFilesOutputMode::FilesWithMatches,
                context: None,
                head_limit: None,
                offset: 0,
                case_insensitive: false,
                include_hidden: false,
            },
            "test_plugin",
        )
        .await?;

    assert!(out.files.as_ref().expect("files").is_empty());
    assert!(out.warnings.iter().any(|w| w.contains("unsupported regex")
        || w.contains("lookaround")
        || w.contains("look-around")));
    Ok(())
}

/// T9 from plan §3.3: Tier2 must skip binary files and oversize text files,
/// surfacing a warning instead of reading them whole.
#[tokio::test]
#[serial(env_lock)]
async fn test_search_files_tier2_skips_binary_and_large_files(
) -> Result<(), Box<dyn std::error::Error>> {
    let tmp = TempDir::new()?;
    let root = tmp.path().join("workspace");
    let bin = tmp.path().join("empty-bin");
    std::fs::create_dir_all(root.join("src"))?;
    std::fs::create_dir_all(&bin)?;
    std::fs::write(root.join("src/text.rs"), "needle\n")?;
    std::fs::write(
        root.join("src/binary.rs"),
        b"prefix\0\0\0needle\nstill binary\n",
    )?;
    let mut big = vec![b'x'; (5 * 1024 * 1024) + 16];
    big.extend_from_slice(b"\nneedle\n");
    std::fs::write(root.join("src/huge.rs"), &big)?;
    let _path = PathGuard::set(&bin);

    let executor = make_executor(&root.canonicalize()?, vec![]);
    let out = executor
        .search_files(
            SearchFilesArgs {
                pattern: "needle".to_string(),
                target: SearchFilesTarget::Content,
                path: Some(root.to_string_lossy().into_owned()),
                glob: None,
                file_type: None,
                output_mode: SearchFilesOutputMode::FilesWithMatches,
                context: None,
                head_limit: None,
                offset: 0,
                case_insensitive: false,
                include_hidden: false,
            },
            "test_plugin",
        )
        .await?;

    let files = out.files.as_ref().expect("files");
    assert_eq!(files, &vec!["src/text.rs"]);
    assert!(out
        .warnings
        .iter()
        .any(|w| w.contains("binary files") || w.contains("larger than")));
    Ok(())
}

/// T10 from plan §3.3: Tier2 `include_hidden=true` must surface dotfile entries
/// just like Tier1 `--hidden`.
#[tokio::test]
#[serial(env_lock)]
async fn test_search_files_tier2_include_hidden_toggle() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = TempDir::new()?;
    let root = tmp.path().join("workspace");
    let bin = tmp.path().join("empty-bin");
    std::fs::create_dir_all(root.join(".hidden"))?;
    std::fs::create_dir_all(&bin)?;
    std::fs::write(root.join(".hidden/secret.rs"), "needle\n")?;
    let _path = PathGuard::set(&bin);

    let executor = make_executor(&root.canonicalize()?, vec![]);

    let visible_only = executor
        .search_files(
            SearchFilesArgs {
                pattern: "*.rs".to_string(),
                target: SearchFilesTarget::Files,
                path: Some(root.to_string_lossy().into_owned()),
                glob: None,
                file_type: None,
                output_mode: SearchFilesOutputMode::Content,
                context: None,
                head_limit: None,
                offset: 0,
                case_insensitive: false,
                include_hidden: false,
            },
            "test_plugin",
        )
        .await?;
    assert!(visible_only.files.as_ref().expect("files").is_empty());

    let with_hidden = executor
        .search_files(
            SearchFilesArgs {
                pattern: "*.rs".to_string(),
                target: SearchFilesTarget::Files,
                path: Some(root.to_string_lossy().into_owned()),
                glob: None,
                file_type: None,
                output_mode: SearchFilesOutputMode::Content,
                context: None,
                head_limit: None,
                offset: 0,
                case_insensitive: false,
                include_hidden: true,
            },
            "test_plugin",
        )
        .await?;
    assert_eq!(
        with_hidden.files.as_ref().expect("files"),
        &vec![".hidden/secret.rs"]
    );
    Ok(())
}

#[tokio::test]
async fn test_search_files_head_limit_validation() -> Result<(), Box<dyn std::error::Error>> {
    let tmp = TempDir::new()?;
    let root = tmp.path().join("workspace");
    std::fs::create_dir_all(&root)?;
    let executor = make_executor(&root.canonicalize()?, vec![]);
    let err = executor
        .search_files(
            SearchFilesArgs {
                pattern: "needle".to_string(),
                target: SearchFilesTarget::Content,
                path: Some(root.to_string_lossy().into_owned()),
                glob: None,
                file_type: None,
                output_mode: SearchFilesOutputMode::FilesWithMatches,
                context: None,
                head_limit: Some(Some(0)),
                offset: 0,
                case_insensitive: false,
                include_hidden: false,
            },
            "test_plugin",
        )
        .await
        .expect_err("head_limit=0 must fail before spawning rg");
    assert!(err.to_string().contains("head_limit"));
    Ok(())
}
