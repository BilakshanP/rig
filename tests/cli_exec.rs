//! Integration tests for execution features: --parallel, --no-parallel,
//! on-return handlers, rig action (sub-configs), and meta.env.

use std::process::Command;

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_rig"))
}

fn write(path: &std::path::Path, contents: &str) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, contents).unwrap();
}

#[test]
fn meta_env_applies_globally() {
    let src = tempfile::tempdir().unwrap();
    write(
        &src.path().join("test.json"),
        if cfg!(windows) {
            r#"{"name":"env-test","version":"1.0.0","meta":{"env":{"MY_VAR":"global"}},"steps":[{"name":"echo","action":{"kind":"shell","commands":["echo %MY_VAR%"]}}]}"#
        } else {
            r#"{"name":"env-test","version":"1.0.0","meta":{"env":{"MY_VAR":"global"}},"steps":[{"name":"echo","action":{"kind":"shell","commands":["echo $MY_VAR"]}}]}"#
        },
    );
    let out = bin()
        .arg(src.path().join("test.json"))
        .arg("-q")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success());
    assert!(
        stdout.contains("global"),
        "meta.env should be available to shell commands"
    );
}

#[test]
fn step_env_overrides_meta_env() {
    let src = tempfile::tempdir().unwrap();
    write(
        &src.path().join("test.json"),
        if cfg!(windows) {
            r#"{"name":"env-test","version":"1.0.0","meta":{"env":{"MY_VAR":"global"}},"steps":[{"name":"echo","action":{"kind":"shell","commands":["echo %MY_VAR%"],"env":{"MY_VAR":"local"}}}]}"#
        } else {
            r#"{"name":"env-test","version":"1.0.0","meta":{"env":{"MY_VAR":"global"}},"steps":[{"name":"echo","action":{"kind":"shell","commands":["echo $MY_VAR"],"env":{"MY_VAR":"local"}}}]}"#
        },
    );
    let out = bin()
        .arg(src.path().join("test.json"))
        .arg("-q")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success());
    assert!(
        stdout.contains("local"),
        "step env should override meta.env"
    );
    assert!(
        !stdout.contains("global"),
        "global value should be overridden"
    );
}

#[test]
fn on_return_fires_with_specific_exit_code() {
    let src = tempfile::tempdir().unwrap();
    write(
        &src.path().join("test.json"),
        r#"{
            "name":"on-return-test","version":"1.0.0",
            "steps":[
                {"name":"exit42","action":{"kind":"shell","commands":["exit 42"]},"on-return":{"42":"h42"}},
                {"id":"h42","name":"handler","action":{"kind":"shell","commands":["echo MATCHED_42"]},"meta":{"optional":true}}
            ]
        }"#,
    );
    let out = bin()
        .arg(src.path().join("test.json"))
        .arg("-q")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "should succeed via handler: {stdout}");
    assert!(stdout.contains("MATCHED_42"), "on-return(42) should fire");
}

#[test]
fn rig_action_runs_sub_config() {
    let src = tempfile::tempdir().unwrap();
    write(
        &src.path().join("sub.json"),
        r#"{"name":"sub","version":"1.0.0","meta":{"vars":{"msg":"default"}},"steps":[{"name":"echo","action":{"kind":"shell","commands":["echo {{msg}}"]}}]}"#,
    );
    let sub_path = src.path().join("sub.json").to_str().unwrap().to_string();
    write(
        &src.path().join("main.json"),
        &format!(
            r#"{{"name":"main","version":"1.0.0","steps":[{{"name":"run-sub","action":{{"kind":"rig","file":"{}","set":{{"msg":"hello"}}}}}}]}}"#,
            sub_path.replace('\\', "\\\\")
        ),
    );
    let out = bin()
        .arg(src.path().join("main.json"))
        .arg("-q")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "rig action should succeed: {stdout}");
    assert!(
        stdout.contains("hello"),
        "sub-config should receive set vars"
    );
}

#[test]
fn parallel_flag_runs_dag_order() {
    let src = tempfile::tempdir().unwrap();
    write(
        &src.path().join("test.json"),
        r#"{"name":"par","version":"1.0.0","steps":[
            {"id":"a","name":"A","action":{"kind":"shell","commands":["echo A"]}},
            {"id":"b","name":"B","depends-on":["a"],"action":{"kind":"shell","commands":["echo B"]}},
            {"id":"c","name":"C","depends-on":["a"],"action":{"kind":"shell","commands":["echo C"]}}
        ]}"#,
    );
    let out = bin()
        .arg(src.path().join("test.json"))
        .arg("--parallel")
        .arg("-q")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success());
    let a_pos = stdout.find('A').unwrap();
    let b_pos = stdout.find('B').unwrap();
    let c_pos = stdout.find('C').unwrap();
    assert!(a_pos < b_pos, "A should run before B");
    assert!(a_pos < c_pos, "A should run before C");
}

#[test]
fn no_parallel_overrides_meta_parallel() {
    let src = tempfile::tempdir().unwrap();
    write(
        &src.path().join("test.json"),
        r#"{"name":"par","version":"1.0.0","meta":{"parallel":true},"steps":[
            {"id":"a","name":"A","action":{"kind":"shell","commands":["echo A"]}},
            {"id":"b","name":"B","depends-on":["a"],"action":{"kind":"shell","commands":["echo B"]}}
        ]}"#,
    );
    let out = bin()
        .arg(src.path().join("test.json"))
        .arg("--no-parallel")
        .arg("-q")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success());
    assert!(stdout.contains("A"));
    assert!(stdout.contains("B"));
    assert!(stdout.find('A').unwrap() < stdout.find('B').unwrap());
}

#[test]
fn exit_action_terminates_early_with_code() {
    let src = tempfile::tempdir().unwrap();
    write(
        &src.path().join("test.json"),
        r#"{"name":"exit-test","version":"1.0.0","steps":[
            {"name":"bail","action":{"kind":"exit","code":0,"message":"done early"}},
            {"name":"never","action":{"kind":"shell","commands":["echo SHOULD_NOT_RUN"]}}
        ]}"#,
    );
    let out = bin().arg(src.path().join("test.json")).output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "exit code 0 should succeed");
    assert!(stdout.contains("done early"), "message should print");
    assert!(
        !stdout.contains("SHOULD_NOT_RUN"),
        "second step should not run"
    );
}

#[test]
fn exit_action_nonzero_code() {
    let src = tempfile::tempdir().unwrap();
    write(
        &src.path().join("test.json"),
        r#"{"name":"exit-test","version":"1.0.0","steps":[
            {"name":"fail","action":{"kind":"exit","code":42,"message":"custom error"}}
        ]}"#,
    );
    let out = bin().arg(src.path().join("test.json")).output().unwrap();
    assert_eq!(out.status.code(), Some(42));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("custom error"));
}

#[test]
fn fragment_selects_subdirectory() {
    let src = tempfile::tempdir().unwrap();
    write(
        &src.path().join("rust/manifest.json"),
        r#"{"name":"rust-tmpl","version":"1.0.0","steps":[
            {"name":"hi","action":{"kind":"io","level":"info","message":"from rust"}}
        ]}"#,
    );
    write(
        &src.path().join("python/manifest.json"),
        r#"{"name":"py-tmpl","version":"1.0.0","steps":[
            {"name":"hi","action":{"kind":"io","level":"info","message":"from python"}}
        ]}"#,
    );

    let out = bin()
        .arg(src.path())
        .arg("--fragment")
        .arg("rust")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "fragment rust failed: {stdout}");
    assert!(stdout.contains("from rust"));

    let out = bin()
        .arg(src.path())
        .arg("--fragment")
        .arg("python")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "fragment python failed: {stdout}");
    assert!(stdout.contains("from python"));
}

#[test]
fn fragment_errors_on_missing_subdir() {
    let src = tempfile::tempdir().unwrap();
    write(
        &src.path().join("rust/manifest.json"),
        r#"{"name":"x","version":"1.0.0","steps":[]}"#,
    );

    let out = bin()
        .arg(src.path())
        .arg("--fragment")
        .arg("nonexistent")
        .output()
        .unwrap();
    assert!(!out.status.success());
}

#[test]
fn meta_on_failure_runs_on_error() {
    let src = tempfile::tempdir().unwrap();
    write(
        &src.path().join("test.json"),
        r#"{"name":"meta-handler","version":"1.0.0",
            "meta":{"on-failure":"cleanup"},
            "steps":[
                {"name":"fail","action":{"kind":"shell","commands":["exit 1"]}},
                {"id":"cleanup","name":"cleanup","action":{"kind":"shell","commands":["echo CLEANED_UP"]},"meta":{"optional":true}}
            ]}"#,
    );
    let out = bin().arg(src.path().join("test.json")).output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(!out.status.success());
    assert!(
        stdout.contains("CLEANED_UP"),
        "on-failure handler should run"
    );
}

#[test]
fn meta_on_success_runs_on_completion() {
    let src = tempfile::tempdir().unwrap();
    write(
        &src.path().join("test.json"),
        r#"{"name":"meta-handler","version":"1.0.0",
            "meta":{"on-success":"done"},
            "steps":[
                {"name":"ok","action":{"kind":"shell","commands":["echo HI"]}},
                {"id":"done","name":"done","action":{"kind":"shell","commands":["echo ALL_DONE"]},"meta":{"optional":true}}
            ]}"#,
    );
    let out = bin().arg(src.path().join("test.json")).output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success());
    assert!(stdout.contains("ALL_DONE"), "on-success handler should run");
}
