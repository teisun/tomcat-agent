mod common;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf()
}

fn canonicalize_for_compare(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

fn parse_shell_array(script: &str, name: &str) -> Vec<String> {
    let start = script
        .find(&format!("{name}=("))
        .unwrap_or_else(|| panic!("missing shell array {name}"));
    let tail = &script[start..];
    let open = tail.find('(').expect("array open paren");
    let close = tail[open + 1..]
        .find("\n)")
        .map(|idx| open + 1 + idx)
        .expect("array close paren");
    tail[open + 1..close]
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(|line| {
            line.split('#')
                .next()
                .unwrap_or_default()
                .trim()
                .to_string()
        })
        .filter(|line| !line.is_empty())
        .collect()
}

fn read_test_groups() -> String {
    std::fs::read_to_string(repo_root().join("scripts/test-groups.sh"))
        .expect("read scripts/test-groups.sh")
}

fn read_nextest_config() -> toml::Value {
    toml::from_str(
        &std::fs::read_to_string(repo_root().join(".config/nextest.toml"))
            .expect("read .config/nextest.toml"),
    )
    .expect("parse nextest config")
}

fn profile_default_filter(config: &toml::Value) -> &str {
    config["profile"]["default"]["default-filter"]
        .as_str()
        .expect("profile.default.default-filter should be a string")
}

fn profile_real_llm_filter(config: &toml::Value) -> &str {
    config["profile"]["real-llm"]["default-filter"]
        .as_str()
        .expect("profile.real-llm.default-filter should be a string")
}

#[test]
fn temp_home_guard_switches_and_restores_home() {
    let original_home = std::env::var_os("HOME");
    let temp_home = {
        let guard = common::TempHomeGuard::new();
        let temp_home = guard.home_path().to_path_buf();
        let temp_dot_tomcat = canonicalize_for_compare(&guard.dot_tomcat_path());
        assert_eq!(
            std::env::var_os("HOME").as_deref(),
            Some(temp_home.as_os_str()),
            "guard 存活期间 HOME 应指向临时目录"
        );
        assert!(
            guard.dot_tomcat_path().join("plans").is_dir(),
            "TempHomeGuard 应预建 ~/.tomcat/plans"
        );
        assert!(
            guard.dot_tomcat_path().join("temp").is_dir(),
            "TempHomeGuard 应预建 ~/.tomcat/temp"
        );
        let workdir = common::dot_tomcat_e2e_workdir("temp_home_guard");
        assert!(
            canonicalize_for_compare(&workdir).starts_with(temp_dot_tomcat.join("temp")),
            "dot_tomcat_e2e_workdir 应落在临时 HOME/.tomcat/temp 下，实际：{}",
            workdir.display()
        );
        temp_home
    };
    assert_eq!(
        std::env::var_os("HOME"),
        original_home,
        "guard Drop 后 HOME 应恢复原值"
    );
    if let Some(home) = original_home {
        assert_ne!(PathBuf::from(home), temp_home);
    }
}

#[test]
fn temp_home_guard_keeps_multiple_workdirs_isolated_under_same_temp_home() {
    let guard = common::TempHomeGuard::new();
    let first = common::dot_tomcat_e2e_workdir("first");
    let second = common::dot_tomcat_e2e_workdir("second");
    let expected_root = canonicalize_for_compare(&guard.dot_tomcat_path()).join("temp");
    assert!(canonicalize_for_compare(&first).starts_with(&expected_root));
    assert!(canonicalize_for_compare(&second).starts_with(&expected_root));
    assert_ne!(first, second, "每次调用都应生成新的唯一 workdir");
}

#[test]
fn promoted_parallel_and_nextest_real_llm_filters_stay_in_sync() {
    let groups = read_test_groups();
    let parallel = parse_shell_array(&groups, "TOMCAT_INTEGRATION_PARALLEL_TESTS");
    let serial = parse_shell_array(&groups, "TOMCAT_INTEGRATION_SERIAL_TESTS");
    let real_llm = parse_shell_array(&groups, "TOMCAT_INTEGRATION_REAL_LLM_TESTS");
    let real_llm_cli = parse_shell_array(&groups, "TOMCAT_INTEGRATION_REAL_LLM_CLI_TESTS");
    let config = read_nextest_config();
    let default_filter = profile_default_filter(&config);
    let real_llm_filter = profile_real_llm_filter(&config);

    let promoted_from_old_serial = [
        "cli_tests",
        "checkpoint_cli_e2e",
        "resume_hydration_cli_e2e",
        "quickjs_e2e_tests",
        "long_lived_vm_tests",
        "hostcall_tests",
        "primitives_tools_tests",
        "tool_catalog_doc",
        "serve_multi_session",
        "serve_ask_question_tests",
        "serve_schema_fixture",
        "serve_robustness_tests",
        "serve_stdio_e2e",
    ];
    let parallel_set = parallel.iter().cloned().collect::<BTreeSet<_>>();
    for binary in promoted_from_old_serial {
        assert!(
            parallel_set.contains(binary),
            "默认并行组应包含原串行 binary `{binary}`"
        );
    }
    assert!(
        serial.is_empty(),
        "serial 兜底组默认应为空，实际：{serial:?}"
    );
    assert!(
        !real_llm_cli.is_empty()
            && real_llm_cli
                .iter()
                .all(|test_name| test_name.ends_with("_real_llm_cli")),
        "real-llm CLI 测试清单应显式维护且全部以 _real_llm_cli 结尾，实际：{real_llm_cli:?}"
    );

    for binary in &real_llm {
        let needle = format!("binary({binary})");
        assert!(
            default_filter.contains(&needle),
            "default filter 应排除显式 real-llm binary `{binary}`"
        );
        assert!(
            real_llm_filter.contains(&needle),
            "real-llm profile 应包含显式 real-llm binary `{binary}`"
        );
    }
    assert!(
        default_filter.contains("test(/_real_llm_cli$/)"),
        "default filter 应排除 cli_tests 里的 *_real_llm_cli 慢用例"
    );
    assert!(
        real_llm_filter.contains("test(/_real_llm_cli$/)"),
        "real-llm profile 应包含 cli_tests 里的 *_real_llm_cli 慢用例"
    );

    assert_eq!(
        config["test-groups"]["serial"]["max-threads"].as_integer(),
        Some(1),
        "serial 兜底组必须保持 max-threads=1"
    );
    assert_eq!(
        config["test-groups"]["real-llm"]["max-threads"].as_integer(),
        Some(2),
        "real-llm profile 应限制为 max-threads=2"
    );
    assert_eq!(
        config["profile"]["real-llm"]["overrides"][0]["test-group"].as_str(),
        Some("real-llm"),
        "real-llm profile 的 override 应绑定到 real-llm test-group"
    );
}
