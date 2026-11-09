//! Offline, isolated CLI checks. English owns wording; both locales preserve behavior.
use assert_cmd::Command;
use std::{ffi::OsString, path::Path, process::Stdio, time::Duration};
use tomcat::infra::tr;

fn command(home: &Path, language: &str) -> Command {
    let mut child = std::process::Command::new(assert_cmd::cargo::cargo_bin("tomcat"));
    child.stdin(Stdio::null());
    let mut cmd = Command::from_std(child);
    for (key, _) in std::env::vars_os() {
        if key.to_string_lossy().starts_with("TOMCAT__") || key == "TOMCAT_HOST_LOCALE" {
            cmd.env_remove(key);
        }
    }
    cmd.current_dir(home)
        .env("HOME", home)
        .env("USERPROFILE", home)
        .env("TOMCAT__UI__LANGUAGE", language)
        .timeout(Duration::from_secs(10));
    for key in [
        "XDG_CONFIG_HOME",
        "XDG_DATA_HOME",
        "XDG_CACHE_HOME",
        "XDG_STATE_HOME",
        "XDG_RUNTIME_DIR",
    ] {
        cmd.env(key, home.join(key));
    }
    cmd
}

fn entries(dir: &Path) -> Vec<OsString> {
    let mut names = std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect::<Vec<_>>();
    names.sort();
    names
}

fn normalized(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn assert_help(home: &Path, language: &str, args: &[&str]) -> String {
    let result = command(home, language)
        .args(args)
        .assert()
        .success()
        .stderr("");
    let text = std::str::from_utf8(&result.get_output().stdout).unwrap();
    assert!(text.contains("--help"));
    let keys: &[&str] = if args.first() == Some(&"acp") {
        for token in [
            "Agent Client Protocol",
            "--skills-dir",
            "--mode",
            "--plan-review",
            "--question-policy",
            "--plan-start",
            "-h",
            "chat",
            "plan",
            "ask, auto, skip",
            "manual, recommended",
            "ask, auto",
        ] {
            assert!(
                text.contains(token),
                "{language} {args:?}: missing {token}\n{text}"
            );
        }
        &[
            "tomcat.acp.long",
            "tomcat.acp.skills_dir",
            "tomcat.acp.mode",
            "tomcat.acp.plan_review",
            "tomcat.acp.question_policy",
            "tomcat.acp.plan_start",
        ]
    } else {
        for token in ["Rust", "tomcat serve", "tomcat acp --help"] {
            assert!(
                text.contains(token),
                "{language} {args:?}: missing {token}\n{text}"
            );
        }
        &["tomcat.long", "tomcat.acp"]
    };
    if language == "en" {
        let catalog: serde_json::Value =
            serde_json::from_str(include_str!("../assets/i18n/cli_help.en.json")).unwrap();
        for key in keys {
            assert!(
                normalized(text).contains(&normalized(catalog[key].as_str().unwrap())),
                "{key}"
            );
        }
        assert!(text.contains(&tr("cli.usage", &[])));
    } else {
        assert!(text.contains("用法"));
    }
    text.to_owned()
}

#[test]
fn help_and_parse_errors_preserve_tokens_without_creating_runtime_files() {
    for language in ["en", "zh-CN"] {
        for args in [&["--help"][..], &["acp", "--help"][..]] {
            let home = tempfile::tempdir().unwrap();
            let before = entries(home.path());
            assert!(before.is_empty());
            let text = assert_help(home.path(), language, args);
            assert_eq!(entries(home.path()), before, "{language} {args:?}");
            println!("=== {language} {args:?}: exit=0, stderr empty, HOME before=[] after=[] ===\n{text}");
        }
        let home = tempfile::tempdir().unwrap();
        let before = entries(home.path());
        assert!(before.is_empty());
        let error = command(home.path(), language)
            .arg("--unrecognized-option")
            .assert()
            .code(2)
            .stderr(predicates::str::contains("--unrecognized-option"));
        if language == "en" {
            error.stderr(predicates::str::contains(tr("cli.error.unknown_arg", &[])));
        }
        assert_eq!(entries(home.path()), before, "{language} unknown option");
    }
}

#[test]
fn help_survives_corrupt_config_and_keeps_version_exit_code() {
    for language in ["en", "zh-CN"] {
        for args in [
            &["--help"][..],
            &["acp", "--help"][..],
            &["session", "delete", "--help"][..],
            &["--version"][..],
        ] {
            let home = tempfile::tempdir().unwrap();
            let config_dir = home.path().join(".tomcat");
            let config = config_dir.join("tomcat.config.toml");
            std::fs::create_dir(&config_dir).unwrap();
            std::fs::write(&config, "invalid [[[ toml").unwrap();
            let before_home = entries(home.path());
            let before_config_dir = entries(&config_dir);
            let before_bytes = std::fs::read(&config).unwrap();
            assert_eq!(before_home, vec![OsString::from(".tomcat")]);
            assert_eq!(
                before_config_dir,
                vec![OsString::from("tomcat.config.toml")]
            );
            if args == ["--help"] || args == ["acp", "--help"] {
                assert_help(home.path(), language, args);
            } else {
                let result = command(home.path(), language)
                    .args(args)
                    .assert()
                    .success()
                    .stderr("");
                if args.first() == Some(&"session") {
                    let result = result.stdout(predicates::str::contains("--scope"));
                    if language == "en" {
                        let catalog: serde_json::Value =
                            serde_json::from_str(include_str!("../assets/i18n/cli_help.en.json"))
                                .unwrap();
                        result.stdout(predicates::str::contains(
                            catalog["tomcat.session.delete"].as_str().unwrap(),
                        ));
                    }
                } else {
                    result.stdout(predicates::str::contains(env!("CARGO_PKG_VERSION")));
                }
            }
            assert_eq!(entries(home.path()), before_home, "{language} {args:?}");
            assert_eq!(
                entries(&config_dir),
                before_config_dir,
                "{language} {args:?}"
            );
            assert_eq!(
                std::fs::read(&config).unwrap(),
                before_bytes,
                "{language} {args:?}"
            );
            println!("corrupt config {language} {args:?}: exit=0, stderr empty, HOME/config listings and config bytes unchanged");
        }
    }
}

#[test]
fn doctor_preserves_read_only_behavior_in_both_languages() {
    for language in ["en", "zh-CN"] {
        let home = tempfile::tempdir().unwrap();
        let result = command(home.path(), language)
            .arg("doctor")
            .assert()
            .success();
        if language == "en" {
            result
                .stdout(predicates::str::contains(tr("cli.doctor.noConfig", &[])))
                .stdout(predicates::str::contains(tr(
                    "cli.doctor.noConfigHint",
                    &[],
                )));
        }
        assert!(!home.path().join(".tomcat").exists());
    }
}

#[test]
fn public_error_shell_preserves_machine_exit_code_in_both_languages() {
    for language in ["en", "zh-CN"] {
        let home = tempfile::tempdir().unwrap();
        let dir = home.path().join(".tomcat");
        std::fs::create_dir(&dir).unwrap();
        std::fs::write(dir.join("tomcat.config.toml"), "invalid [[[ toml").unwrap();
        let error = command(home.path(), language)
            .args(["config", "get", "ui.language"])
            .assert()
            .code(1);
        if language == "en" {
            error
                .stderr(predicates::str::contains(tr("cli.error", &[])))
                .stderr(predicates::str::contains(tr("error.prefix.config", &[])));
        }
    }
}
