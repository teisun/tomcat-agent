use super::super::bash_parser::extract_paths;

#[test]
fn extracts_absolute_path() {
    assert_eq!(extract_paths("cat /etc/passwd"), vec!["/etc/passwd"]);
}

#[test]
fn plain_relative_path_without_prefix_is_not_extracted() {
    assert!(extract_paths("rm src/main.rs").is_empty());
}

#[test]
fn extracts_dot_relative() {
    assert_eq!(extract_paths("ls ./build"), vec!["./build"]);
}

#[test]
fn extracts_tilde_path() {
    assert_eq!(extract_paths("cat ~/.bashrc"), vec!["~/.bashrc"]);
}

#[test]
fn skips_flags() {
    let v = extract_paths("ls -la /tmp");
    assert_eq!(v, vec!["/tmp"]);
}

#[test]
fn extracts_flag_value_paths() {
    let v = extract_paths("cargo --target-dir=/tmp/target build");
    assert_eq!(v, vec!["/tmp/target"]);
}

#[test]
fn extracts_assignment_in_arg_position() {
    let v = extract_paths("stat -c %s p=/Users/a/file");
    assert_eq!(v, vec!["/Users/a/file"]);
}

#[test]
fn extracts_leading_env_assignment_before_cmd() {
    let v = extract_paths("p=/Users/a/file ls -la \"$p\"");
    assert!(v.contains(&"/Users/a/file".to_string()));
}

#[test]
fn extracts_leading_env_assignment_in_subcommand() {
    let v = extract_paths("p=/Users/a/file; cmd $p");
    assert!(v.contains(&"/Users/a/file".to_string()));
}

#[test]
fn keeps_existing_flag_value_behavior() {
    let v = extract_paths("cargo --target-dir=/tmp/target build");
    assert_eq!(v, vec!["/tmp/target"]);
}

#[test]
fn ignores_empty_rhs() {
    let v = extract_paths("p= cmd");
    assert!(v.is_empty());
}

#[test]
fn ignores_non_identifier_lhs() {
    let v = extract_paths("echo 123=/path");
    assert!(v.is_empty());
}

#[test]
fn multiple_leading_assignments() {
    let v = extract_paths("A=/x B=/y cmd");
    assert_eq!(v, vec!["/x", "/y"]);
}

#[test]
fn handles_pipes_and_subcommands() {
    let v = extract_paths("cat /etc/hosts | grep 127.0.0.1 > /tmp/out");
    // pipe 把命令拆成 [cat /etc/hosts, grep 127.0.0.1, /tmp/out]
    // 第三段 ">" 之后只剩 "/tmp/out" 整段；第一个 token 被当作命令名跳过 -> 不提取。
    // 重定向目标 intentionally 不做路径 gate（见 bash_parser 模块 TODO）。
    assert!(v.contains(&"/etc/hosts".to_string()));
    assert!(!v.contains(&"/tmp/out".to_string()));
}

#[test]
fn handles_quoted_strings() {
    let v = extract_paths("rm \"my file.txt\" /tmp/x");
    assert!(v.contains(&"/tmp/x".to_string()));
}

#[test]
fn no_longer_treats_plain_slash_tokens_as_paths() {
    let v = extract_paths("npm i -D @playwright/test && node -e \"console.log('http://x/y')\"");
    assert!(v.is_empty(), "legacy helper 只应识别显式路径前缀: {:?}", v);
}
