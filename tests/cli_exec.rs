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
