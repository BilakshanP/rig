# Contributing to rig

## Setup

```bash
git clone https://github.com/BilakshanP/rig.git
cd rig
cargo build
cargo test
```

## Development

```bash
cargo run -- examples/dev-env.jsonc --dry-run   # test a config
cargo clippy                                     # lint
cargo test                                       # run all tests
```

### Cross-compile for Windows (from Linux)

```bash
rustup target add x86_64-pc-windows-gnu
sudo apt install gcc-mingw-w64-x86-64    # Debian/Ubuntu
sudo dnf install mingw64-gcc             # Fedora
cargo build --release --target x86_64-pc-windows-gnu
```

## Project Structure

```
src/
  main.rs       — CLI entry point + dispatch logic
  config.rs     — Config/Step/Action types, parsing, validation
  executor.rs   — Step execution, dry-run audit, retry logic
  bundle.rs     — .rig archive format, git clone, directory bundles
  vars.rs       — Variable system (scopes, substitution, built-ins)
  inspect.rs    — --list and --describe display
  path.rs       — Tilde expansion
  style.rs      — Colored output (aml)
tests/
  cli_bundle.rs — Integration tests (compiled binary)
```

## Adding a New Action Kind

1. Add variant to `Action` enum in `config.rs`
2. Add to `exec_action` match in `executor.rs`
3. Add to `audit_step` match in `executor.rs` (dry-run)
4. Add to the match in `inspect.rs` (--describe)
5. Add to `collect_refs_in_action` in `config.rs` (variable scanning)
6. Add to `visit_step_refs` if it contains step references (validation)
7. Add to `schema.json` (with `additionalProperties: false`)
8. Add a test
9. Update README, CONTEXT.md, AGENTS.md

## Releases

Releases are automated via GitHub Actions on tag push.

### Creating a release

```bash
# 1. Bump version in Cargo.toml
# 2. Commit
git add Cargo.toml Cargo.lock
git commit -m "chore: bump version to vX.Y.Z"

# 3. Tag and push
git tag vX.Y.Z
git push && git push origin vX.Y.Z
```

CI will automatically:
- Build binaries for Linux (x86_64), Windows (x86_64), macOS (x86_64 + aarch64)
- Create a GitHub release with auto-generated notes
- Attach all binaries

### Manual release (if needed)

```bash
cargo build --release
cargo build --release --target x86_64-pc-windows-gnu

gh release create vX.Y.Z \
  target/release/rig \
  target/x86_64-pc-windows-gnu/release/rig.exe \
  --title "vX.Y.Z" \
  --generate-notes
```

## Testing

- Unit tests: `cargo test` (in-process, no binary needed)
- Integration tests: `tests/cli_bundle.rs` (exercises the compiled binary)
- Validate examples: `cargo run -- examples/<file> --validate`

All tests must pass on both Linux and Windows (CI runs both).

## Code Style

- Follow existing patterns — match the surrounding code
- `cargo clippy` must pass with no warnings
- Keep `additionalProperties: false` on all schema action definitions
- Update docs alongside code changes (README, CONTEXT.md, AGENTS.md, schema.json)
