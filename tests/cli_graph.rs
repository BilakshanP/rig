//! Integration tests for --graph and --dot output.

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
fn graph_shows_edges() {
    let src = tempfile::tempdir().unwrap();
    write(
        &src.path().join("test.json"),
        r#"{"name":"g","version":"1.0.0","steps":[
            {"id":"a","name":"A","action":{"kind":"shell","commands":["echo a"]}},
            {"id":"b","name":"B","depends-on":["a"],"action":{"kind":"shell","commands":["echo b"]}}
        ]}"#,
    );
    let out = bin()
        .arg(src.path().join("test.json"))
        .arg("--graph")
        .output()
        .unwrap();
    let stdout = strip_ansi(&String::from_utf8_lossy(&out.stdout));
    assert!(out.status.success());
    assert!(stdout.contains("a"), "graph should list step a");
    assert!(stdout.contains("b"), "graph should list step b");
}

#[test]
fn graph_dot_output() {
    let src = tempfile::tempdir().unwrap();
    write(
        &src.path().join("test.json"),
        r#"{"name":"g","version":"1.0.0","steps":[
            {"id":"a","name":"A","action":{"kind":"shell","commands":["echo a"]}},
            {"id":"b","name":"B","depends-on":["a"],"action":{"kind":"shell","commands":["echo b"]}}
        ]}"#,
    );
    let out = bin()
        .arg(src.path().join("test.json"))
        .arg("--graph")
        .arg("--dot")
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success());
    assert!(stdout.contains("digraph"), "should output DOT format");
    assert!(
        stdout.contains("\"b\" -> \"a\""),
        "should have depends-on edge"
    );
}
