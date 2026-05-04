//! Integration tests for the `rig pack` / `rig unpack` / `rig info` subcommands.
//!
//! These exercise the compiled binary (via `CARGO_BIN_EXE_rig`) so we verify
//! the clap wiring, not just the library functions.

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
fn pack_and_unpack_roundtrip() {
    let src = tempfile::tempdir().unwrap();
    write(
        &src.path().join("manifest.json"),
        r#"{"name":"cli-test","version":"0.0.1","steps":[]}"#,
    );
    write(&src.path().join("data/hello.txt"), "hi from cli");

    let archive_dir = tempfile::tempdir().unwrap();
    let archive = archive_dir.path().join("cli.rig");

    let status = bin()
        .arg("pack")
        .arg(src.path())
        .arg("-o")
        .arg(&archive)
        .status()
        .unwrap();
    assert!(status.success(), "pack subcommand failed");
    assert!(archive.is_file());

    let dst = tempfile::tempdir().unwrap();
    let status = bin()
        .arg("unpack")
        .arg(&archive)
        .arg("-o")
        .arg(dst.path())
        .status()
        .unwrap();
    assert!(status.success(), "unpack subcommand failed");

    assert!(dst.path().join("manifest.json").is_file());
    assert_eq!(
        std::fs::read_to_string(dst.path().join("data/hello.txt")).unwrap(),
        "hi from cli"
    );
}

#[test]
fn info_prints_manifest_summary() {
    let src = tempfile::tempdir().unwrap();
    write(
        &src.path().join("manifest.jsonc"),
        r#"{
            // header comment
            "name": "cli-info-demo",
            "version": "2.3.4",
            "description": "test description",
            "steps": [
                {"name":"a","action":{"kind":"shell","commands":["echo a"]}}
            ]
        }"#,
    );
    write(&src.path().join("assets/blob.bin"), "payload");

    let archive_dir = tempfile::tempdir().unwrap();
    let archive = archive_dir.path().join("info.rig");

    let status = bin()
        .arg("pack")
        .arg(src.path())
        .arg("-o")
        .arg(&archive)
        .status()
        .unwrap();
    assert!(status.success());

    let output = bin().arg("info").arg(&archive).output().unwrap();
    assert!(output.status.success(), "info subcommand failed");
    let stdout = String::from_utf8_lossy(&output.stdout);
    // Strip ANSI for easier matching.
    let plain = strip_ansi(&stdout);
    assert!(plain.contains("cli-info-demo"), "output missing name: {plain}");
    assert!(plain.contains("v2.3.4"), "output missing version: {plain}");
    assert!(plain.contains("test description"), "output missing description: {plain}");
    assert!(plain.contains("steps: 1"), "output missing step count: {plain}");
    assert!(plain.contains("manifest.jsonc"), "output missing manifest entry: {plain}");
    assert!(plain.contains("assets/blob.bin"), "output missing nested file: {plain}");
}

#[test]
fn pack_errors_without_manifest() {
    let src = tempfile::tempdir().unwrap();
    write(&src.path().join("just-a-file.txt"), "no manifest");
    let archive_dir = tempfile::tempdir().unwrap();
    let archive = archive_dir.path().join("bad.rig");

    let output = bin()
        .arg("pack")
        .arg(src.path())
        .arg("-o")
        .arg(&archive)
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        strip_ansi(&stderr).contains("manifest"),
        "expected manifest error, got: {stderr}"
    );
}

#[test]
fn invoking_without_args_or_subcommand_fails() {
    let output = bin().output().unwrap();
    assert!(!output.status.success(), "rig with no args should fail");
}

#[test]
fn running_a_trivial_bundle_succeeds() {
    // End-to-end smoke test: a bundle whose only step is an io banner. This
    // exercises the full bundle-detection → open_bundle → Runner path
    // without depending on filesystem semantics we refine in later tasks.
    let src = tempfile::tempdir().unwrap();
    write(
        &src.path().join("manifest.jsonc"),
        r#"{
            "name": "smoke",
            "version": "0.0.1",
            "bundle": { "extract-to": "tmp", "cleanup": "always" },
            "steps": [
                {"name":"banner","action":{"kind":"io","level":"info","message":"from bundle"}}
            ]
        }"#,
    );

    let archive_dir = tempfile::tempdir().unwrap();
    let archive = archive_dir.path().join("smoke.rig");
    let status = bin()
        .arg("pack")
        .arg(src.path())
        .arg("-o")
        .arg(&archive)
        .status()
        .unwrap();
    assert!(status.success());

    let output = bin().arg(&archive).output().unwrap();
    assert!(
        output.status.success(),
        "running bundle failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let plain = strip_ansi(&String::from_utf8_lossy(&output.stdout));
    assert!(
        plain.contains("from bundle"),
        "expected banner text, got: {plain}"
    );
}

fn strip_ansi(s: &str) -> String {
    String::from_utf8(strip_ansi_escapes::strip(s)).unwrap_or_else(|_| s.to_string())
}
