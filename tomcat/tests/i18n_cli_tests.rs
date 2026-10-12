//! Offline, isolated CLI checks. English owns wording; both locales preserve behavior.
use assert_cmd::Command;
use tomcat::infra::tr;

fn command(home: &std::path::Path, language: &str) -> Command {
    let mut cmd = Command::cargo_bin("tomcat").unwrap();
    cmd.env("HOME", home)
        .env("USERPROFILE", home)
        .env("TOMCAT__UI__LANGUAGE", language)
        .env_remove("TOMCAT_HOST_LOCALE");
    cmd
}

#[test]
fn help_and_parse_errors_preserve_tokens_without_creating_runtime_files() {
    for language in ["en", "zh-CN"] {
        let home = tempfile::tempdir().unwrap();
        let help = command(home.path(), language)
            .arg("--help")
            .assert()
            .success()
            .stdout(predicates::str::contains("--help"));
        if language == "en" {
            help.stdout(predicates::str::contains(tr("cli.usage", &[])));
        }
        let error = command(home.path(), language)
            .arg("--unrecognized-option")
            .assert()
            .code(2)
            .stderr(predicates::str::contains("--unrecognized-option"));
        if language == "en" {
            error.stderr(predicates::str::contains(tr("cli.error.unknown_arg", &[])));
        }
        assert!(!home.path().join(".tomcat").exists());
    }
}

#[test]
fn help_survives_corrupt_config_and_keeps_version_exit_code() {
    let home = tempfile::tempdir().unwrap();
    let config_dir = home.path().join(".tomcat");
    std::fs::create_dir(&config_dir).unwrap();
    std::fs::write(config_dir.join("tomcat.config.toml"), "invalid [[[ toml").unwrap();
    let english_help: serde_json::Value =
        serde_json::from_str(include_str!("../assets/i18n/cli_help.en.json")).unwrap();
    command(home.path(), "en")
        .args(["session", "delete", "--help"])
        .assert()
        .success()
        .stdout(predicates::str::contains(
            english_help["tomcat.session.delete"].as_str().unwrap(),
        ));
    command(home.path(), "zh-CN")
        .args(["session", "delete", "--help"])
        .assert()
        .success()
        .stdout(predicates::str::contains("--scope"));
    command(home.path(), "en")
        .arg("--version")
        .assert()
        .success();
    assert_eq!(std::fs::read_dir(&config_dir).unwrap().count(), 1);
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
