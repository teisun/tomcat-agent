//! `tests/cwd_lazy_prompt_e2e.rs`：T2-P0-004 hotfix §A 集成测试。
//!
//! 通过 `tomcat::api::chat::cwd_lazy_prompt::CwdLazyPrompt` 装配真实
//! `DefaultPermissionGate` + `SessionGrants` + 真实 toml 文件，验证：
//!
//! 1. 非 TTY（cargo test 默认）下，cwd 内首次 `confirm_decision` 走 fallback +
//!    `dismissed=true`，避免 CI 阻塞；
//! 2. dismissed 一旦置位，后续 confirm_decision 不再尝试范围级提示；
//! 3. 直接驱动 `apply_choice_for_test`：`[a]` 真的写盘 `workspace_roots`、`[s]`
//!    只动 SessionGrants、`[c]` 设 dismissed 并拒绝当前操作；
//! 4. cwd 已被 SessionGrants 包含时，装饰器整段不介入。
//!
//! 真正基于真实 PTY/stdin 的交互式回放走「§E.3 手工验收脚本」，由人在终端
//! 执行；这里只覆盖可被自动化稳定通过的部分。

use std::path::PathBuf;
use std::sync::Arc;

use tomcat::api::chat::permission::cwd_lazy::{CwdLazyPrompt, CwdPromptChoice};
use tomcat::core::permission::{DefaultPermissionGate, GateConfig, PermissionGate, SessionGrants};
use tomcat::core::tools::contract::confirmation::ConfirmDecision;
use tomcat::{
    AllowAllConfirmation, DenyAllConfirmation, PrimitiveOperation, UserConfirmationProvider,
};

fn make_gate(definition: &std::path::Path) -> Arc<dyn PermissionGate> {
    let cfg = GateConfig {
        agent_definition_dir: definition.to_path_buf(),
        workspace_roots: vec![],
        agent_trail_readonly_dirs: vec![],
        user_path_rules: vec![],
        user_bash_forbidden: vec![],
        user_bash_approval: vec![],
        auto_confirm: false,
    };
    Arc::new(DefaultPermissionGate::new(cfg, SessionGrants::new()))
}

fn write_minimal_config(cfg_path: &std::path::Path) {
    let toml = r#"
[agent]
id = "main"

[llm]
default_model = "gpt-5.2"

[workspace]
workspace_roots = []

[primitive]
auto_confirm = false
"#;
    std::fs::write(cfg_path, toml).unwrap();
}

fn build_preview(target: &str) -> String {
    format!("[Read] 读取\n路径: {}\n原因: 不在已授权范围内", target)
}

#[tokio::test]
async fn non_tty_first_touch_sets_dismissed_and_forwards() {
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path().join("cwd");
    std::fs::create_dir_all(&cwd).unwrap();
    let cfg_path = tmp.path().join("tomcat.config.toml");
    write_minimal_config(&cfg_path);

    let gate = make_gate(&PathBuf::from("/__nowhere__"));
    let inner: Arc<dyn UserConfirmationProvider> = Arc::new(DenyAllConfirmation);
    let session_grants = SessionGrants::new();
    let prompt = CwdLazyPrompt::new(inner, cwd.clone(), gate, session_grants.clone(), cfg_path);

    let preview = build_preview(&cwd.join("foo.txt").to_string_lossy());
    let dec = prompt
        .confirm_decision(PrimitiveOperation::Read, &preview, "__agent__", None)
        .await
        .unwrap();
    assert_eq!(dec, ConfirmDecision::Deny);
    assert!(prompt.is_dismissed());
    assert!(
        session_grants.snapshot().is_empty(),
        "非 TTY 路径不应写 SessionGrants"
    );
}

#[tokio::test]
async fn second_call_after_dismissed_skips_lazy_branch() {
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path().join("cwd");
    std::fs::create_dir_all(&cwd).unwrap();
    let cfg_path = tmp.path().join("tomcat.config.toml");
    write_minimal_config(&cfg_path);

    let gate = make_gate(&PathBuf::from("/__nowhere__"));
    let inner: Arc<dyn UserConfirmationProvider> = Arc::new(AllowAllConfirmation);
    let prompt = CwdLazyPrompt::new(inner, cwd.clone(), gate, SessionGrants::new(), cfg_path);

    // 1) 非 TTY → dismissed=true
    let preview = build_preview(&cwd.join("foo.txt").to_string_lossy());
    let _ = prompt
        .confirm_decision(PrimitiveOperation::Read, &preview, "__agent__", None)
        .await
        .unwrap();
    assert!(prompt.is_dismissed());

    // 2) 第二次进入 → dismissed 早返回 → AllowAll 直接放行
    let preview2 = build_preview(&cwd.join("bar.txt").to_string_lossy());
    let dec2 = prompt
        .confirm_decision(PrimitiveOperation::Read, &preview2, "__agent__", None)
        .await
        .unwrap();
    assert_eq!(
        dec2,
        ConfirmDecision::AllowOnce,
        "dismissed 后应直接走 inner（这里 AllowAll → AllowOnce）"
    );
}

#[tokio::test]
async fn add_persistent_choice_writes_workspace_roots_and_grants() {
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path().join("cwd");
    std::fs::create_dir_all(&cwd).unwrap();
    let cfg_path = tmp.path().join("tomcat.config.toml");
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

    // toml 校验
    let canon = std::fs::canonicalize(&cwd).unwrap();
    let toml_text = std::fs::read_to_string(&cfg_path).unwrap();
    assert!(
        toml_text.contains(canon.to_string_lossy().as_ref()),
        "workspace_roots 应包含 cwd canonical 路径，实际:\n{}",
        toml_text
    );

    // SessionGrants 校验
    assert!(
        session_grants.snapshot().iter().any(|p| p == &canon),
        "[a] 必须同时写 SessionGrants"
    );

    // 不应触发 dismissed
    assert!(!prompt.is_dismissed());
}

#[tokio::test]
async fn allow_session_only_choice_does_not_write_disk() {
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path().join("cwd");
    std::fs::create_dir_all(&cwd).unwrap();
    let cfg_path = tmp.path().join("tomcat.config.toml");
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
    let _ = prompt
        .apply_choice_for_test(
            CwdPromptChoice::AllowSessionOnly,
            PrimitiveOperation::Read,
            &preview,
            "__agent__",
            None,
        )
        .await
        .unwrap();

    let toml_after = std::fs::read_to_string(&cfg_path).unwrap();
    assert_eq!(toml_after, toml_before, "[s] 不应改 toml");

    let canon = std::fs::canonicalize(&cwd).unwrap();
    assert!(session_grants.snapshot().iter().any(|p| p == &canon));
}

#[tokio::test]
async fn cancel_choice_sets_dismissed_and_denies_current_operation() {
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path().to_path_buf();
    let cfg_path = tmp.path().join("tomcat.config.toml");
    write_minimal_config(&cfg_path);

    let gate = make_gate(&PathBuf::from("/__nowhere__"));
    let inner: Arc<dyn UserConfirmationProvider> = Arc::new(DenyAllConfirmation);
    let session_grants = SessionGrants::new();
    let prompt = CwdLazyPrompt::new(inner, cwd.clone(), gate, session_grants.clone(), cfg_path);

    let preview = build_preview(&cwd.join("foo.txt").to_string_lossy());
    let dec = prompt
        .apply_choice_for_test(
            CwdPromptChoice::Cancel,
            PrimitiveOperation::Read,
            &preview,
            "__agent__",
            None,
        )
        .await
        .unwrap();
    assert_eq!(dec, ConfirmDecision::Deny);
    assert!(prompt.is_dismissed());
    assert!(session_grants.snapshot().is_empty());
}

#[tokio::test]
async fn cwd_already_authorized_bypasses_decorator_entirely() {
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tmp.path().to_path_buf();
    let cfg_path = tmp.path().join("tomcat.config.toml");
    write_minimal_config(&cfg_path);

    // gate.workspace_dir == cwd ⇒ effective_roots.read_write 包含 cwd
    let gate = make_gate(&cwd);
    let inner: Arc<dyn UserConfirmationProvider> = Arc::new(AllowAllConfirmation);
    let prompt = CwdLazyPrompt::new(inner, cwd.clone(), gate, SessionGrants::new(), cfg_path);

    let preview = build_preview(&cwd.join("foo.txt").to_string_lossy());
    let dec = prompt
        .confirm_decision(PrimitiveOperation::Read, &preview, "__agent__", None)
        .await
        .unwrap();
    assert_eq!(dec, ConfirmDecision::AllowOnce);
    assert!(
        !prompt.is_dismissed(),
        "短路 cwd_authorized 不应触碰 dismissed"
    );
}
