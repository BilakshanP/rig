//! Integration tests for output control: -q, -qq, -s, and their combinations.

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

fn strip_ansi(s: &str) -> String {
    String::from_utf8(strip_ansi_escapes::strip(s)).unwrap_or_else(|_| s.to_string())
}

#[test]
fn quiet_suppresses_chrome() {
    let src = tempfile::tempdir().unwrap();
    write(
        &src.path().join("test.json"),
        r#"{"name":"q-test","version":"1.0.0","steps":[{"name":"echo","action":{"kind":"shell","commands":["echo hello"]}}]}"#,
    );
    let out = bin()
        .arg(src.path().join("test.json"))
        .arg("-q")
        .output()
        .unwrap();
    let stdout = strip_ansi(&String::from_utf8_lossy(&out.stdout));
    assert!(out.status.success());
    assert!(!stdout.contains("Running:"), "chrome should be suppressed");
    assert!(!stdout.contains("->"), "step arrows should be suppressed");
    assert!(!stdout.contains("Done."), "Done should be suppressed");
    assert!(stdout.contains("hello"), "command output should show");
}

#[test]
fn quiet_qq_suppresses_all_output() {
    let src = tempfile::tempdir().unwrap();
    write(
        &src.path().join("test.json"),
        r#"{"name":"qq-test","version":"1.0.0","steps":[{"name":"echo","action":{"kind":"shell","commands":["echo hello"]}},{"name":"msg","action":{"kind":"io","level":"info","message":"hi"}}]}"#,
    );
    let out = bin()
        .arg(src.path().join("test.json"))
        .arg("-qq")
        .output()
        .unwrap();
    let stdout = strip_ansi(&String::from_utf8_lossy(&out.stdout));
    assert!(out.status.success());
    assert!(
        !stdout.contains("hello"),
        "command output should be suppressed at -qq"
    );
    assert!(
        !stdout.contains("hi"),
        "io messages should be suppressed at -qq"
    );
}

#[test]
fn silent_suppresses_command_output_but_keeps_chrome() {
    let src = tempfile::tempdir().unwrap();
    write(
        &src.path().join("test.json"),
        r#"{"name":"s-test","version":"1.0.0","steps":[{"name":"echo","action":{"kind":"shell","commands":["echo hello"]}}]}"#,
    );
    let out = bin()
        .arg(src.path().join("test.json"))
        .arg("-s")
        .output()
        .unwrap();
    let stdout = strip_ansi(&String::from_utf8_lossy(&out.stdout));
    assert!(out.status.success());
    assert!(stdout.contains("->"), "chrome should show with --silent");
    assert!(stdout.contains("Done."), "Done should show with --silent");
    assert!(
        !stdout.contains("hello"),
        "command output should be suppressed with --silent"
    );
}

#[test]
fn quiet_and_silent_together_suppresses_everything() {
    let src = tempfile::tempdir().unwrap();
    write(
        &src.path().join("test.json"),
        r#"{"name":"qs-test","version":"1.0.0","steps":[{"name":"echo","action":{"kind":"shell","commands":["echo hello"]}}]}"#,
    );
    let out = bin()
        .arg(src.path().join("test.json"))
        .arg("-q")
        .arg("-s")
        .output()
        .unwrap();
    let stdout = strip_ansi(&String::from_utf8_lossy(&out.stdout));
    assert!(out.status.success());
    assert!(
        stdout.trim().is_empty(),
        "both -q and -s should produce no output"
    );
}
