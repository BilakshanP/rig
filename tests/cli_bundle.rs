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
fn pack_without_output_flag_derives_rig_suffix() {
    let src = tempfile::tempdir().unwrap();
    write(
        &src.path().join("manifest.json"),
        r#"{"name":"auto","version":"0.0.1","steps":[]}"#,
    );
    // Source dir is named by its tempdir suffix; pack it from a scratch cwd
    // so the derived output lands somewhere we control.
    let work = tempfile::tempdir().unwrap();
    let copy_dir = work.path().join("auto");
    std::fs::create_dir_all(&copy_dir).unwrap();
    std::fs::copy(src.path().join("manifest.json"), copy_dir.join("manifest.json")).unwrap();

    let status = bin()
        .current_dir(work.path())
        .arg("pack")
        .arg("auto")
        .status()
        .unwrap();
    assert!(status.success(), "pack without --output failed");
    assert!(
        work.path().join("auto.rig").is_file(),
        "expected auto.rig in cwd"
    );
}

#[test]
fn unpack_without_output_flag_strips_rig_suffix() {
    let src = tempfile::tempdir().unwrap();
    write(
        &src.path().join("manifest.json"),
        r#"{"name":"auto","version":"0.0.1","steps":[]}"#,
    );
    let work = tempfile::tempdir().unwrap();
    let archive = work.path().join("thing.rig");
    let status = bin()
        .arg("pack")
        .arg(src.path())
        .arg("-o")
        .arg(&archive)
        .status()
        .unwrap();
    assert!(status.success());

    let status = bin()
        .current_dir(work.path())
        .arg("unpack")
        .arg("thing.rig")
        .status()
        .unwrap();
    assert!(status.success(), "unpack without --output failed");
    // Should have created ./thing/ (the .rig suffix stripped) in cwd.
    assert!(
        work.path().join("thing").join("manifest.json").is_file(),
        "expected thing/manifest.json after default unpack"
    );
}

#[test]
fn unpack_errors_when_no_rig_suffix_and_no_output() {
    let src = tempfile::tempdir().unwrap();
    write(
        &src.path().join("manifest.json"),
        r#"{"name":"auto","version":"0.0.1","steps":[]}"#,
    );
    let work = tempfile::tempdir().unwrap();
    let archive = work.path().join("archive.tgz"); // deliberate: no .rig
    let status = bin()
        .arg("pack")
        .arg(src.path())
        .arg("-o")
        .arg(&archive)
        .status()
        .unwrap();
    assert!(status.success());

    let output = bin()
        .current_dir(work.path())
        .arg("unpack")
        .arg("archive.tgz")
        .output()
        .unwrap();
    assert!(!output.status.success());
    let stderr = strip_ansi(&String::from_utf8_lossy(&output.stderr));
    assert!(
        stderr.contains(".rig suffix"),
        "expected suffix error, got: {stderr}"
    );
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

/// Shared fixture: a bundle with two steps and a required var so we can
/// exercise every inspection flag.
fn pack_inspection_bundle() -> (tempfile::TempDir, std::path::PathBuf) {
    let src = tempfile::tempdir().unwrap();
    write(
        &src.path().join("manifest.jsonc"),
        r#"{
            "name": "inspect-demo",
            "version": "1.2.3",
            "description": "fixture for bundle inspection flags",
            "meta": { "vars": { "greeting": "hi" } },
            "bundle": { "extract-to": "tmp", "cleanup": "always" },
            "steps": [
                {
                    "id": "banner",
                    "name": "banner step",
                    "action": {"kind":"io","level":"info","message":"{{greeting}} {{who}}"}
                },
                {
                    "id": "noop",
                    "name": "noop step",
                    "action": {"kind":"shell","commands":["true"]}
                }
            ]
        }"#,
    );
    let archive_dir = tempfile::tempdir().unwrap();
    let archive = archive_dir.path().join("inspect.rig");
    let status = bin().arg("pack").arg(src.path()).arg("-o").arg(&archive).status().unwrap();
    assert!(status.success());
    (archive_dir, archive)
}

#[test]
fn bundle_validate_succeeds_without_set() {
    let (_guard, archive) = pack_inspection_bundle();
    let output = bin().arg("--validate").arg(&archive).output().unwrap();
    assert!(output.status.success(), "--validate failed: {}", String::from_utf8_lossy(&output.stderr));
    assert!(strip_ansi(&String::from_utf8_lossy(&output.stdout)).contains("ok:"));
}

#[test]
fn bundle_list_succeeds_without_set() {
    let (_guard, archive) = pack_inspection_bundle();
    let output = bin().arg("--list").arg(&archive).output().unwrap();
    assert!(output.status.success());
    let plain = strip_ansi(&String::from_utf8_lossy(&output.stdout));
    assert!(plain.contains("banner"), "missing banner id in --list: {plain}");
    assert!(plain.contains("noop"), "missing noop id in --list: {plain}");
}

#[test]
fn bundle_describe_succeeds_without_set() {
    let (_guard, archive) = pack_inspection_bundle();
    let output = bin().arg("--describe").arg("banner").arg(&archive).output().unwrap();
    assert!(output.status.success());
    let plain = strip_ansi(&String::from_utf8_lossy(&output.stdout));
    assert!(plain.contains("banner step"), "describe output: {plain}");
    // Raw template preserved (no substitution in describe).
    assert!(plain.contains("{{greeting}}"), "expected raw template in describe: {plain}");
}

#[test]
fn bundle_vars_lists_undefined() {
    let (_guard, archive) = pack_inspection_bundle();
    let output = bin().arg("--vars").arg(&archive).output().unwrap();
    assert!(output.status.success(), "--vars failed: {}", String::from_utf8_lossy(&output.stderr));
    let plain = strip_ansi(&String::from_utf8_lossy(&output.stdout));
    assert!(plain.contains("greeting"), "missing greeting: {plain}");
    assert!(plain.contains("who"), "missing who: {plain}");
    assert!(plain.contains("(required)"), "'who' should be marked required: {plain}");
}

#[test]
fn bundle_dry_run_requires_set_vars() {
    let (_guard, archive) = pack_inspection_bundle();
    // Without --set, the undefined var should error (matches pre-bundle behavior).
    let output = bin().arg("--dry-run").arg(&archive).output().unwrap();
    assert!(!output.status.success());
    let stderr = strip_ansi(&String::from_utf8_lossy(&output.stderr));
    assert!(stderr.contains("undefined variable"), "expected var error, got: {stderr}");
}

#[test]
fn bundle_dry_run_succeeds_with_set() {
    let (_guard, archive) = pack_inspection_bundle();
    let output = bin()
        .arg("--dry-run")
        .arg(&archive)
        .arg("--set")
        .arg("who=world")
        .output()
        .unwrap();
    assert!(output.status.success(), "--dry-run failed: {}", String::from_utf8_lossy(&output.stderr));
    let plain = strip_ansi(&String::from_utf8_lossy(&output.stdout));
    assert!(plain.contains("[dry-run]"), "missing dry-run tag: {plain}");
    assert!(plain.contains("inspect-demo"), "missing config name: {plain}");
}

#[test]
fn bundle_only_runs_single_step() {
    let (_guard, archive) = pack_inspection_bundle();
    let output = bin()
        .arg("--only")
        .arg("banner")
        .arg(&archive)
        .arg("--set")
        .arg("who=world")
        .output()
        .unwrap();
    assert!(output.status.success(), "--only failed: {}", String::from_utf8_lossy(&output.stderr));
    let plain = strip_ansi(&String::from_utf8_lossy(&output.stdout));
    assert!(plain.contains("hi world"), "rendered message missing: {plain}");
    // The other step ("noop") should NOT have been recorded in output —
    // --only runs exactly one step.
    assert!(!plain.contains("noop step"), "--only ran more than one step: {plain}");
}

#[test]
fn python_project_example_bundle_renders_templates() {
    // End-to-end: pack the real python-project example, run the file-copy
    // steps against a tempdir, verify the output tree is rendered correctly.
    // The shell steps (git init / uv sync / basedpyright) are skipped —
    // they require external tools not guaranteed in CI.

    let manifest = env!("CARGO_MANIFEST_DIR");
    let src_dir = std::path::PathBuf::from(manifest).join("examples/python-project");
    assert!(src_dir.is_dir(), "missing example: {}", src_dir.display());

    let archive_dir = tempfile::tempdir().unwrap();
    let archive = archive_dir.path().join("python-project.rig");
    let status = bin().arg("pack").arg(&src_dir).arg("-o").arg(&archive).status().unwrap();
    assert!(status.success(), "pack failed");

    let run_dir = tempfile::tempdir().unwrap();
    let file_steps = [
        "create-dirs",
        "pyproject",
        "pyrightconfig",
        "python-version",
        "gitignore",
        "init",
        "main",
        "cli",
        "test",
        "ci",
    ];
    for step in file_steps {
        let output = bin()
            .current_dir(run_dir.path())
            .arg("--only")
            .arg(step)
            .arg(&archive)
            .arg("--set")
            .arg("name=my-tool")
            .arg("--set")
            .arg("package=my_tool")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "step '{step}' failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    // Spot-check the tree.
    let root = run_dir.path().join("my-tool");
    assert!(root.is_dir(), "expected output root: {}", root.display());

    let pyproject = std::fs::read_to_string(root.join("pyproject.toml")).unwrap();
    assert!(pyproject.contains("name = \"my-tool\""), "pyproject not rendered: {pyproject}");
    assert!(pyproject.contains("src/my_tool"), "pyproject package path missing: {pyproject}");

    let cli_py = std::fs::read_to_string(root.join("src/my_tool/cli.py")).unwrap();
    assert!(cli_py.contains("Hello from my-tool"), "cli.py not rendered: {cli_py}");

    let test_py = std::fs::read_to_string(root.join("tests/test_cli.py")).unwrap();
    assert!(test_py.contains("import my_tool.cli"), "test_cli.py not rendered: {test_py}");

    assert!(root.join(".python-version").is_file());
    assert!(root.join(".gitignore").is_file());
    assert!(root.join(".github/workflows/ci.yml").is_file());
    assert!(root.join("pyrightconfig.json").is_file());
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
    assert!(!stdout.contains("hello"), "command output should be suppressed at -qq");
    assert!(!stdout.contains("hi"), "io messages should be suppressed at -qq");
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
    assert!(!stdout.contains("hello"), "command output should be suppressed with --silent");
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
    assert!(stdout.trim().is_empty(), "both -q and -s should produce no output");
}
