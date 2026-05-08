//! `permission::bash_ast` 的单测——切段 / allow / deny / unsupported / 子 shell 等。
//!
//! 测试只触达 `BashAstChecker` / `AstReject` / `AstSegmentVerdict` /
//! `NoopSandboxBackend` 等公共 API；与 [`super::super::bash_ast`] 模块文档保持
//! 一致的「与 scope spec §2/§4 对齐」语义。

use super::super::bash_ast::{
    AstReject, AstSegmentVerdict, BashAstChecker, NoopSandboxBackend, SandboxBackend,
};

fn checker_with(allow: &[&str], deny: &[&str]) -> BashAstChecker {
    BashAstChecker::new(
        true,
        allow.iter().map(|s| s.to_string()).collect(),
        deny.iter().map(|s| s.to_string()).collect(),
    )
}

#[test]
fn disabled_checker_returns_single_defer_segment() {
    let chk = BashAstChecker::default();
    let v = chk.check("rm -rf /; cd /").expect("disabled 应当 Ok");
    assert_eq!(v.len(), 1, "enabled=false 应当只产出 1 段（整条命令）");
    assert_eq!(v[0].1, AstSegmentVerdict::Defer);
    assert_eq!(v[0].0.raw, "rm -rf /; cd /");
}

#[test]
fn split_on_semicolon_and_short_circuit_and_pipe() {
    let chk = checker_with(&[], &[]);
    let v = chk
        .check("git pull && rm -rf node_modules; ls -la | wc -l")
        .unwrap();
    let cmds: Vec<String> = v.iter().map(|(s, _)| s.command.clone()).collect();
    assert_eq!(cmds, vec!["git", "rm", "ls", "wc"]);
}

#[test]
fn deny_short_circuits_remaining_segments() {
    let chk = checker_with(&[], &["rm"]);
    let err = chk.check("git pull && rm -rf node_modules").unwrap_err();
    match err {
        AstReject::AstDeny { command, .. } => assert_eq!(command, "rm"),
        other => panic!("expected AstDeny, got {:?}", other),
    }
}

#[test]
fn allow_marks_skip_approval_but_still_yields_defer_for_others() {
    let chk = checker_with(&["ls"], &[]);
    let v = chk.check("ls -la; cat README").unwrap();
    assert_eq!(v[0].1, AstSegmentVerdict::AllowedSkipApproval);
    assert_eq!(v[1].1, AstSegmentVerdict::Defer);
}

#[test]
fn assignment_prefix_is_kept_as_segment_attribute() {
    let chk = checker_with(&["env"], &[]);
    let v = chk.check("FOO=bar BAZ=qux env").unwrap();
    assert_eq!(v.len(), 1);
    assert_eq!(v[0].0.command, "env");
    assert_eq!(v[0].0.assignments, vec!["FOO=bar", "BAZ=qux"]);
}

#[test]
fn subshell_command_substitution_is_treated_as_opaque_literal() {
    // PR-L MVP 不递归解析子 shell 内部命令；外层命令照常判定，内部按字面量带过。
    let chk = checker_with(&["printf"], &[]);
    let v = chk
        .check("printf '%s\\n' $(seq 1 3); echo done")
        .expect("MVP 不应拒子 shell 表面语法");
    assert_eq!(v.len(), 2);
    assert_eq!(v[0].0.command, "printf");
    assert_eq!(v[0].1, AstSegmentVerdict::AllowedSkipApproval);
    assert_eq!(v[1].0.command, "echo");
    assert_eq!(v[1].1, AstSegmentVerdict::Defer);
}

#[test]
fn separators_inside_subshell_do_not_split_segments() {
    let chk = checker_with(&[], &[]);
    let v = chk.check("echo $(a; b && c | d) post").unwrap();
    assert_eq!(v.len(), 1, "子 shell 内的分隔符不应触发外层切段");
    assert_eq!(v[0].0.command, "echo");
}

#[test]
fn unmatched_subshell_returns_parse_error() {
    let chk = checker_with(&[], &[]);
    let err = chk.check("echo $(seq 1 3").unwrap_err();
    assert!(matches!(err, AstReject::ParseError { .. }));
}

#[test]
fn flow_control_keywords_are_unsupported() {
    let chk = checker_with(&[], &[]);
    for cmd in [
        "for i in 1 2 3; do echo $i; done",
        "if [ -f a ]; then ls; fi",
        "while true; do sleep 1; done",
    ] {
        let res = chk.check(cmd);
        assert!(
            matches!(res, Err(AstReject::Unsupported { .. })),
            "expected Unsupported for `{}`, got {:?}",
            cmd,
            res
        );
    }
}

#[test]
fn heredoc_is_unsupported() {
    let chk = checker_with(&[], &[]);
    let err = chk.check("cat <<EOF\nhi\nEOF").unwrap_err();
    assert!(matches!(err, AstReject::Unsupported { .. }));
}

#[test]
fn unmatched_quote_returns_parse_error() {
    let chk = checker_with(&[], &[]);
    let err = chk.check("echo 'hi").unwrap_err();
    assert!(matches!(err, AstReject::ParseError { .. }));
}

#[test]
fn quoted_separators_do_not_split_segments() {
    let chk = checker_with(&[], &[]);
    let v = chk.check("echo 'a; b && c | d'").unwrap();
    assert_eq!(v.len(), 1, "引号内的分隔符不应触发切段");
    assert_eq!(v[0].0.command, "echo");
}

#[test]
fn glob_prefix_pattern_matches() {
    let chk = checker_with(&["git*"], &[]);
    let v = chk.check("git status; git push").unwrap();
    assert_eq!(v[0].1, AstSegmentVerdict::AllowedSkipApproval);
    assert_eq!(v[1].1, AstSegmentVerdict::AllowedSkipApproval);
}

#[tokio::test]
async fn noop_sandbox_backend_spawns_directly() {
    let backend = NoopSandboxBackend;
    let mut cmd = tokio::process::Command::new("echo");
    cmd.arg("hello").stdout(std::process::Stdio::piped());
    let child = backend.spawn(cmd).await.expect("spawn");
    let output = child.wait_with_output().await.expect("wait");
    assert_eq!(output.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&output.stdout).contains("hello"));
    assert_eq!(backend.name(), "noop");
}
