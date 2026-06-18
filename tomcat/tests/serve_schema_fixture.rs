mod common;

use std::fs;
use std::process::Command;

use serial_test::serial;

use common::serve::{cargo_bin_path, fixture_path, setup_serve_fixture};

#[test]
#[serial]
fn serve_print_schema_matches_fixture() {
    common::setup_logging();
    let fx = setup_serve_fixture("http://127.0.0.1:1");
    let out_dir = fx.home_path.join(".tomcat").join("schema-out");

    let output = Command::new(cargo_bin_path())
        .env_remove("TOMCAT__LLM__DEFAULT_MODEL")
        .env("HOME", &fx.home_path)
        .env("SHELL", "/bin/zsh")
        .env("TOMCAT__SERVE__SCHEMA_OUT_DIR", &out_dir)
        .args(["serve", "--print-schema"])
        .output()
        .expect("run serve --print-schema");
    assert!(
        output.status.success(),
        "serve --print-schema should succeed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let actual_schema =
        fs::read_to_string(out_dir.join("serve.schema.json")).expect("read generated schema");
    let actual_dts = fs::read_to_string(out_dir.join("serve.d.ts")).expect("read generated dts");
    let expected_schema = fs::read_to_string(fixture_path(&[
        "tests",
        "fixtures",
        "serve",
        "serve.schema.json",
    ]))
    .expect("read schema fixture");
    let expected_dts =
        fs::read_to_string(fixture_path(&["tests", "fixtures", "serve", "serve.d.ts"]))
            .expect("read dts fixture");

    assert_eq!(actual_schema, expected_schema);
    assert_eq!(actual_dts, expected_dts);
}
