use super::*;

#[test]
fn environment_snapshot_validates_wire_values_and_redacts_debug() {
    let env = EnvironmentSnapshot::from_values(BTreeMap::from([
        ("TASK_TOKEN".into(), "private-fixture-value".into()),
        ("PATH".into(), "/task/bin".into()),
    ]))
    .unwrap();
    assert_eq!(env.get("TASK_TOKEN"), Some("private-fixture-value"));
    assert!(!format!("{env:?}").contains("private-fixture-value"));
    let wire = serde_json::to_string(&env).unwrap();
    let restored: EnvironmentSnapshot = serde_json::from_str(&wire).unwrap();
    restored.validate().unwrap();
    assert_eq!(restored.get("PATH"), Some("/task/bin"));
    for (key, value) in [("", "x"), ("A=B", "x"), ("A\0B", "x"), ("A", "x\0y")] {
        assert!(
            EnvironmentSnapshot::from_values(BTreeMap::from([(key.into(), value.into())])).is_err()
        );
    }
}

#[test]
#[serial_test::serial(env_lock)]
fn environment_snapshot_replaces_explicit_environment_without_changing_process() {
    let before = std::env::var_os("PATH");
    let env =
        EnvironmentSnapshot::from_values(BTreeMap::from([("TASK_VALUE".into(), "task".into())]))
            .unwrap();
    let mut command = std::process::Command::new("unused");
    command.env("STARTER_VALUE", "starter");
    env.apply(&mut command);
    let values = command.get_envs().collect::<Vec<_>>();
    assert_eq!(values.len(), 1);
    assert_eq!(values[0].0, "TASK_VALUE");
    assert_eq!(values[0].1, Some(std::ffi::OsStr::new("task")));
    assert_eq!(std::env::var_os("PATH"), before);
}

#[test]
fn environment_home_paths_do_not_fall_back_to_the_service_home() {
    let root = tempfile::tempdir().unwrap();
    let env = EnvironmentSnapshot::from_values(BTreeMap::from([(
        "HOME".into(),
        root.path().to_string_lossy().into_owned(),
    )]))
    .unwrap();
    assert_eq!(env.expand_home("~/file").unwrap(), root.path().join("file"));
    let native_home_path = format!("~{}file", std::path::MAIN_SEPARATOR);
    assert_eq!(
        env.expand_home(&native_home_path).unwrap(),
        root.path().join("file")
    );
    assert!(EnvironmentSnapshot::default()
        .expand_home(&native_home_path)
        .is_err());
    assert_eq!(
        env.expand_home("relative/file").unwrap(),
        PathBuf::from("relative/file")
    );
    assert!(EnvironmentSnapshot::default().expand_home("~").is_err());
}

#[cfg(windows)]
#[test]
fn environment_program_resolution_uses_child_path_pathext_and_never_service_fallback() {
    let root = tempfile::tempdir().unwrap();
    let a = root.path().join("a");
    let b = root.path().join("b");
    std::fs::create_dir(&a).unwrap();
    std::fs::create_dir(&b).unwrap();
    std::fs::write(a.join("fixture.cmd"), "").unwrap();
    std::fs::write(b.join("fixture.cmd"), "").unwrap();
    let env = EnvironmentSnapshot::from_values(
        [
            ("Path".into(), "a".into()),
            ("PATHEXT".into(), ".CMD".into()),
        ]
        .into(),
    )
    .unwrap();
    assert_eq!(
        env.resolve_program("fixture", root.path(), &BTreeMap::new())
            .unwrap(),
        a.join("fixture.cmd")
    );
    assert_eq!(
        env.resolve_program(
            "fixture",
            root.path(),
            &[("PATH".into(), "b".into())].into()
        )
        .unwrap(),
        b.join("fixture.cmd")
    );
    assert!(env
        .resolve_program(
            "fixture",
            root.path(),
            &[("PATHEXT".into(), ".EXE".into())].into()
        )
        .is_err());
    assert!(EnvironmentSnapshot::default()
        .resolve_program("cmd", root.path(), &BTreeMap::new())
        .is_err());
    assert!(EnvironmentSnapshot::from_values(
        [("PATH".into(), "a".into()), ("Path".into(), "b".into())].into()
    )
    .is_err());
}

#[cfg(windows)]
#[test]
fn connection_git_bash_discovery_is_not_cached_across_controllers() {
    for _ in 0..2 {
        let root = tempfile::tempdir().unwrap();
        let bash = root.path().join("Git/bin/bash.exe");
        std::fs::create_dir_all(bash.parent().unwrap()).unwrap();
        std::fs::write(&bash, "").unwrap();
        let env = EnvironmentSnapshot::from_values(
            [(
                "ProgramFiles".into(),
                root.path().to_string_lossy().into_owned(),
            )]
            .into(),
        )
        .unwrap();
        assert_eq!(
            crate::infra::platform::posix_shell_for(Some(&env), root.path()).unwrap(),
            bash
        );
    }
}
