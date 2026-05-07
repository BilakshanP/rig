//! Integration test: validate all examples against schema.json to detect drift.

use std::process::Command;

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_rig"))
}

/// Validate examples against schema.json (catches schema too strict).
#[test]
fn examples_conform_to_schema() {
    let schema_str = std::fs::read_to_string("schema.json").expect("schema.json not found");
    let schema: serde_json::Value =
        serde_json::from_str(&schema_str).expect("schema.json is not valid JSON");
    let validator = jsonschema::validator_for(&schema).expect("invalid JSON Schema");

    for entry in std::fs::read_dir("examples").expect("examples/ not found") {
        let path = entry.unwrap().path();
        if path.extension().map_or(true, |e| e != "jsonc") {
            continue;
        }
        let content = std::fs::read_to_string(&path).unwrap();
        let mut buf = Vec::new();
        std::io::Read::read_to_end(
            &mut json_comments::StripComments::new(content.as_bytes()),
            &mut buf,
        )
        .unwrap();
        let json: serde_json::Value =
            serde_json::from_slice(&buf).unwrap_or_else(|e| panic!("{}: {e}", path.display()));

        let result = validator.validate(&json);
        if let Err(e) = result {
            panic!("{} failed schema validation: {e}", path.display());
        }
    }
}

/// Validate the comprehensive schema-coverage fixture against both schema.json and rig --validate.
#[test]
fn schema_coverage_fixture_validates() {
    let schema_str = std::fs::read_to_string("schema.json").expect("schema.json not found");
    let schema: serde_json::Value =
        serde_json::from_str(&schema_str).expect("schema.json is not valid JSON");
    let validator = jsonschema::validator_for(&schema).expect("invalid JSON Schema");

    let path = "tests/fixtures/schema-coverage.json";
    let content = std::fs::read_to_string(path).expect("schema-coverage.json not found");
    let json: serde_json::Value =
        serde_json::from_str(&content).unwrap_or_else(|e| panic!("{path}: {e}"));

    if let Err(e) = validator.validate(&json) {
        panic!("{path} failed schema validation: {e}");
    }

    let out = bin().arg(path).arg("--validate").output().unwrap();
    assert!(
        out.status.success(),
        "{path} failed rig --validate: {}",
        String::from_utf8_lossy(&out.stdout)
    );
}

/// Validate examples with `rig --validate` (catches schema too loose).
#[test]
fn examples_pass_rig_validate() {
    for entry in std::fs::read_dir("examples").expect("examples/ not found") {
        let path = entry.unwrap().path();
        if path.extension().map_or(true, |e| e != "jsonc") {
            continue;
        }
        let out = bin().arg(&path).arg("--validate").output().unwrap();
        assert!(
            out.status.success(),
            "{} failed rig --validate: {}",
            path.display(),
            String::from_utf8_lossy(&out.stderr)
        );
    }
}
