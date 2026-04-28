use super::{
    extract_target_from_preview, parse_choice, target_in_cwd, CwdLazyPrompt, CwdPromptChoice,
};
use crate::core::confirmation::{
    AllowAllConfirmation, ConfirmDecision, DenyAllConfirmation, UserConfirmationProvider,
};
use crate::core::permission::{
    DefaultPermissionGate, DraggedPaths, GateConfig, PermissionGate, SessionGrants,
};
use crate::core::primitives::PrimitiveOperation;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

// ── parse_choice ──

#[test]
fn parse_choice_recognizes_aliases() {
    assert_eq!(parse_choice("a"), Some(CwdPromptChoice::AddPersistent));
    assert_eq!(parse_choice("ADD"), Some(CwdPromptChoice::AddPersistent));
    assert_eq!(
        parse_choice("persist"),
        Some(CwdPromptChoice::AddPersistent)
    );
    assert_eq!(parse_choice("s"), Some(CwdPromptChoice::AllowSessionOnly));
    assert_eq!(
        parse_choice("Session"),
        Some(CwdPromptChoice::AllowSessionOnly)
    );
    assert_eq!(
        parse_choice("once"),
        Some(CwdPromptChoice::AllowSessionOnly)
    );
    assert_eq!(parse_choice("n"), Some(CwdPromptChoice::Skip));
    assert_eq!(parse_choice("NO"), Some(CwdPromptChoice::Skip));
    assert_eq!(parse_choice("skip"), Some(CwdPromptChoice::Skip));
    assert_eq!(parse_choice(""), None);
    assert_eq!(parse_choice("xyz"), None);
}

// ── target_in_cwd ──

#[test]
fn target_in_cwd_self_is_true() {
    let cwd = PathBuf::from("/Users/yan/work");
    assert!(target_in_cwd(&cwd, &cwd));
}

#[test]
fn target_in_cwd_subdir_is_true() {
    let cwd = PathBuf::from("/Users/yan/work");
    assert!(target_in_cwd(
        &PathBuf::from("/Users/yan/work/sub/file.txt"),
        &cwd
    ));
}

#[test]
fn target_in_cwd_outside_is_false() {
    let cwd = PathBuf::from("/Users/yan/work");
    assert!(!target_in_cwd(&PathBuf::from("/etc/hosts"), &cwd));
    assert!(!target_in_cwd(&PathBuf::from("/Users/yan"), &cwd));
    // Sibling that shares prefix string but not directory boundary
    assert!(!target_in_cwd(
        &PathBuf::from("/Users/yan/work-sibling/file"),
        &cwd
    ));
}

// ── extract_target_from_preview ──

#[test]
fn extract_target_from_preview_finds_path_line() {
    let preview = "[Read] 读取\n路径: /Users/yan/work/file.txt\n原因: ...";
    assert_eq!(
        extract_target_from_preview(preview),
        Some(PathBuf::from("/Users/yan/work/file.txt"))
    );
}

#[test]
fn extract_target_from_preview_missing_returns_none() {
    let preview = "no path here\nsome other content";
    assert!(extract_target_from_preview(preview).is_none());
}

#[test]
fn extract_target_from_preview_blank_returns_none() {
    let preview = "[Bash] 执行命令\n路径: \n原因: ...";
    assert!(extract_target_from_preview(preview).is_none());
}

// ── decorator behavior（异步 + tempdir 集成）──

fn make_gate(workspace: &Path) -> Arc<dyn PermissionGate> {
    let cfg = GateConfig {
        workspace_dir: workspace.to_path_buf(),
        extra_roots: vec![],
        agent_data_readonly_dirs: vec![],
        user_path_rules: vec![],
        user_bash_forbidden: vec![],
        user_bash_approval: vec![],
        user_bash_whitelist: vec![],
        auto_confirm: false,
    };
    Arc::new(DefaultPermissionGate::new(
        cfg,
        SessionGrants::new(),
        DraggedPaths::new(),
    ))
}

fn build_preview(path: &str) -> String {
    format!("[Read] 读取\n路径: {}\n原因: 不在已授权范围内", path)
}

#[tokio::test]
async fn forwards_when_target_outside_cwd() {
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path().join("inside");
    std::fs::create_dir_all(&cwd).unwrap();
    let gate = make_gate(tmp.path()); // workspace == tempdir, cwd 是 tempdir/inside
    let inner: Arc<dyn UserConfirmationProvider> = Arc::new(DenyAllConfirmation);
    let prompt = CwdLazyPrompt::new(
        inner,
        cwd.clone(),
        gate,
        SessionGrants::new(),
        PathBuf::new(),
    );
    let preview = build_preview("/etc/hosts");
    let dec = prompt
        .confirm_decision(
            PrimitiveOperation::Read,
            &preview,
            "__agent__",
            Some(PathBuf::from("/etc")),
        )
        .await
        .unwrap();
    assert_eq!(dec, ConfirmDecision::Deny, "应直接走 inner DenyAll");
}

#[tokio::test]
async fn forwards_when_dismissed() {
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path().to_path_buf();
    let gate = make_gate(&PathBuf::from("/__nowhere__")); // workspace 在 cwd 之外，cwd 未授权
    let inner: Arc<dyn UserConfirmationProvider> = Arc::new(AllowAllConfirmation);
    let dismissed = Arc::new(AtomicBool::new(true));
    let prompt = CwdLazyPrompt::new(
        inner,
        cwd.clone(),
        gate,
        SessionGrants::new(),
        PathBuf::new(),
    )
    .with_dismissed(dismissed);
    let preview = build_preview(&cwd.join("foo.txt").to_string_lossy());
    let dec = prompt
        .confirm_decision(PrimitiveOperation::Read, &preview, "__agent__", None)
        .await
        .unwrap();
    assert_eq!(
        dec,
        ConfirmDecision::AllowOnce,
        "dismissed=true 时直接走 inner（这里 AllowAll）"
    );
}

#[tokio::test]
async fn forwards_for_bash_op() {
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path().to_path_buf();
    let gate = make_gate(&PathBuf::from("/__nowhere__"));
    let inner: Arc<dyn UserConfirmationProvider> = Arc::new(DenyAllConfirmation);
    let prompt = CwdLazyPrompt::new(inner, cwd, gate, SessionGrants::new(), PathBuf::new());
    let preview = "[Bash] 危险命令命中确认列表\n命令: rm -rf /\n原因: ...".to_string();
    let dec = prompt
        .confirm_decision(PrimitiveOperation::Bash, &preview, "__agent__", None)
        .await
        .unwrap();
    assert_eq!(dec, ConfirmDecision::Deny, "Bash op 不走 cwd 范围分支");
}

#[tokio::test]
async fn forwards_when_cwd_already_authorized() {
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path().to_path_buf();
    // workspace_dir == cwd ⇒ effective_roots.read_write 包含 cwd
    let gate = make_gate(&cwd);
    let inner: Arc<dyn UserConfirmationProvider> = Arc::new(AllowAllConfirmation);
    let prompt = CwdLazyPrompt::new(
        inner,
        cwd.clone(),
        gate,
        SessionGrants::new(),
        PathBuf::new(),
    );
    let preview = build_preview(&cwd.join("foo.txt").to_string_lossy());
    let dec = prompt
        .confirm_decision(PrimitiveOperation::Read, &preview, "__agent__", None)
        .await
        .unwrap();
    assert_eq!(
        dec,
        ConfirmDecision::AllowOnce,
        "cwd 已在 effective_roots 中应直接走 inner"
    );
}

#[tokio::test]
async fn forwards_when_preview_lacks_path_line() {
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path().to_path_buf();
    let gate = make_gate(&PathBuf::from("/__nowhere__"));
    let inner: Arc<dyn UserConfirmationProvider> = Arc::new(DenyAllConfirmation);
    let prompt = CwdLazyPrompt::new(inner, cwd, gate, SessionGrants::new(), PathBuf::new());
    let preview = "config_tool 删除已存在 key 的预览，不带 路径: 行";
    let dec = prompt
        .confirm_decision(PrimitiveOperation::Edit, preview, "__agent__", None)
        .await
        .unwrap();
    assert_eq!(dec, ConfirmDecision::Deny);
}

// ── apply_choice：[a] / [s] / [n] 三分支副作用 ──

fn write_minimal_config(cfg_path: &Path) {
    let toml = r#"
[agent]
id = "main"

[llm]
default_model = "gpt-4o-mini"

[workspace]
extra_roots = []

[primitive]
auto_confirm = false
"#;
    std::fs::write(cfg_path, toml).unwrap();
}

#[tokio::test]
async fn apply_choice_add_persistent_writes_disk_and_session_grants() {
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path().join("cwd");
    std::fs::create_dir_all(&cwd).unwrap();
    let cfg_path = tmp.path().join("pi.config.toml");
    write_minimal_config(&cfg_path);

    let gate = make_gate(&PathBuf::from("/__nowhere__"));
    let inner: Arc<dyn UserConfirmationProvider> = Arc::new(DenyAllConfirmation);
    let session_grants = SessionGrants::new();
    let prompt = CwdLazyPrompt::new(
        inner,
        cwd.clone(),
        gate,
        session_grants.clone(),
        cfg_path.clone(),
    );

    let preview = build_preview(&cwd.join("foo.txt").to_string_lossy());
    let dec = prompt
        .apply_choice_for_test(
            CwdPromptChoice::AddPersistent,
            PrimitiveOperation::Read,
            &preview,
            "__agent__",
            None,
        )
        .await
        .unwrap();
    assert_eq!(dec, ConfirmDecision::AllowOnce);

    // 校验 toml 写盘
    let toml_after = std::fs::read_to_string(&cfg_path).unwrap();
    let canon = std::fs::canonicalize(&cwd).unwrap();
    assert!(
        toml_after.contains(canon.to_string_lossy().as_ref()),
        "extra_roots 应写入 cwd canonical 路径，实际:\n{}",
        toml_after
    );

    // 校验 SessionGrants
    let snap = session_grants.snapshot();
    assert!(
        snap.iter().any(|p| p == &canon),
        "session_grants 应包含 cwd canonical 路径"
    );

    // dismissed 不应被触发
    assert!(!prompt.is_dismissed(), "[a] 不应设置 dismissed");
}

#[tokio::test]
async fn apply_choice_allow_session_only_does_not_write_disk() {
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path().join("cwd");
    std::fs::create_dir_all(&cwd).unwrap();
    let cfg_path = tmp.path().join("pi.config.toml");
    write_minimal_config(&cfg_path);
    let toml_before = std::fs::read_to_string(&cfg_path).unwrap();

    let gate = make_gate(&PathBuf::from("/__nowhere__"));
    let inner: Arc<dyn UserConfirmationProvider> = Arc::new(DenyAllConfirmation);
    let session_grants = SessionGrants::new();
    let prompt = CwdLazyPrompt::new(
        inner,
        cwd.clone(),
        gate,
        session_grants.clone(),
        cfg_path.clone(),
    );

    let preview = build_preview(&cwd.join("foo.txt").to_string_lossy());
    let dec = prompt
        .apply_choice_for_test(
            CwdPromptChoice::AllowSessionOnly,
            PrimitiveOperation::Read,
            &preview,
            "__agent__",
            None,
        )
        .await
        .unwrap();
    assert_eq!(dec, ConfirmDecision::AllowOnce);

    // toml 不应被改写
    let toml_after = std::fs::read_to_string(&cfg_path).unwrap();
    assert_eq!(toml_after, toml_before, "[s] 不应写盘");

    // SessionGrants 应包含 cwd
    let canon = std::fs::canonicalize(&cwd).unwrap();
    assert!(
        session_grants.snapshot().iter().any(|p| p == &canon),
        "[s] 应将 cwd 加入 SessionGrants"
    );
}

#[tokio::test]
async fn apply_choice_skip_sets_dismissed_and_forwards() {
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path().to_path_buf();
    let cfg_path = tmp.path().join("pi.config.toml");
    write_minimal_config(&cfg_path);

    let gate = make_gate(&PathBuf::from("/__nowhere__"));
    let inner: Arc<dyn UserConfirmationProvider> = Arc::new(DenyAllConfirmation);
    let session_grants = SessionGrants::new();
    let prompt = CwdLazyPrompt::new(inner, cwd.clone(), gate, session_grants.clone(), cfg_path);

    let preview = build_preview(&cwd.join("foo.txt").to_string_lossy());
    let dec = prompt
        .apply_choice_for_test(
            CwdPromptChoice::Skip,
            PrimitiveOperation::Read,
            &preview,
            "__agent__",
            None,
        )
        .await
        .unwrap();
    // [n] 后转发给 DenyAll
    assert_eq!(dec, ConfirmDecision::Deny);
    assert!(prompt.is_dismissed(), "[n] 必须设 dismissed=true");
    assert!(
        session_grants.snapshot().is_empty(),
        "[n] 不应改 SessionGrants"
    );
}

#[tokio::test]
async fn apply_choice_add_persistent_is_idempotent_for_session_grants() {
    // 确保连续两次 [a]（如多次 NeedConfirm）不会重复堆叠 SessionGrants 项。
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path().join("cwd");
    std::fs::create_dir_all(&cwd).unwrap();
    let cfg_path = tmp.path().join("pi.config.toml");
    write_minimal_config(&cfg_path);

    let gate = make_gate(&PathBuf::from("/__nowhere__"));
    let inner: Arc<dyn UserConfirmationProvider> = Arc::new(DenyAllConfirmation);
    let session_grants = SessionGrants::new();
    let prompt = CwdLazyPrompt::new(inner, cwd.clone(), gate, session_grants.clone(), cfg_path);
    let preview = build_preview(&cwd.join("foo.txt").to_string_lossy());

    for _ in 0..2 {
        let _ = prompt
            .apply_choice_for_test(
                CwdPromptChoice::AddPersistent,
                PrimitiveOperation::Read,
                &preview,
                "__agent__",
                None,
            )
            .await
            .unwrap();
    }

    let canon = std::fs::canonicalize(&cwd).unwrap();
    let snap = session_grants.snapshot();
    let count = snap.iter().filter(|p| **p == canon).count();
    assert_eq!(count, 1, "SessionGrants 必须去重");
}

#[tokio::test]
async fn dismisses_and_forwards_when_stdin_not_tty() {
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path().to_path_buf();
    let gate = make_gate(&PathBuf::from("/__nowhere__"));
    let inner: Arc<dyn UserConfirmationProvider> = Arc::new(DenyAllConfirmation);
    let prompt = CwdLazyPrompt::new(
        inner,
        cwd.clone(),
        gate,
        SessionGrants::new(),
        PathBuf::new(),
    );
    let preview = build_preview(&cwd.join("foo.txt").to_string_lossy());
    // 测试环境下 stdin 大概率不是 TTY；verify dismissed 路径生效
    let dec = prompt
        .confirm_decision(PrimitiveOperation::Read, &preview, "__agent__", None)
        .await
        .unwrap();
    assert_eq!(dec, ConfirmDecision::Deny);
    assert!(
        prompt.dismissed.load(Ordering::Acquire),
        "非 TTY 路径必须设 dismissed=true"
    );
}
