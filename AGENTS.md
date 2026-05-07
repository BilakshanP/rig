# Agent Instructions for rig

Read `CONTEXT.md` first for full project context.

## Current State

- Fully implemented with `.rig` bundle format, manifest+templates, `expand` flag on fs actions, and bundle-aware `fs.copy` content rendering
- Git repo URLs and local directories as input (shallow-clone + manifest lookup)
- 159 tests passing (136 unit + 23 integration), clippy clean
- JSONC support via `json_comments` crate
- Bundle I/O via `tar` + `flate2` + `tempfile` + `globset`
- HTTP fetching via `ureq`, password input via `rpassword`, home dir via `dirs`

## File Structure

```
schema.json          — JSON Schema (draft-07) for editor validation/autocompletion
examples/
  dev-env.jsonc      — Annotated example with all features
  bootstrap.jsonc    — Example: install Rust, clone repo, build rig
  vars-demo.jsonc    — Variable system walkthrough
  kitchen-sink.jsonc — Broad feature coverage
  python-project/    — Bundle source tree (pack with `rig pack`)
    manifest.jsonc
    {{name}}/...     — templated payload files (literal `{{name}}` dirname)
src/
  main.rs            — CLI entry point (clap derive) + orchestration + subcommands
  config.rs          — Config/Step/Action types + parse_config() + validation + build_scope()
                       + `bundle: Option<BundleMeta>` + `ExpandFlags` on every FsOp
  executor.rs        — Runner with execute dispatch, dry-run audit, retry tracking, runtime subst,
                       bundle-aware fs.copy content rendering
  bundle.rs          — .rig archive format: pack/unpack/info, open_bundle, open_directory,
                       open_git_repo, looks_like_git_repo, clone_repo, BundleCtx + Drop-based
                       cleanup, BinaryMatcher (globset)
  inspect.rs         — --list, --describe, and --graph/--dot/--edges display logic
  vars.rs            — Variable system: VarRef parser, Scope (with #bundle), substitution with
                       {{{{...}}}} escape
  path.rs            — tilde expansion helper
  style.rs           — aml wrapper for colored output
tests/
  cli_bundle.rs      — Integration tests exercising the compiled binary
```

## Data Model

- `Config` — `name`, `version`, optional `description`, `meta: Meta`, `bundle: Option<BundleMeta>`, `steps: Vec<Step>`
- `Meta` — `log?`, `silent: Vec<Silent>`, `sudo`, `retries?`, `retry_delay?`, `shell?: ShellConfig`, `vars: serde_json::Value` (nested)
- `Step` — `id?`, `name`, `description?`, `action: Action`, `depends_on: Vec<String>`, `on_success?: Vec<StepRef>`, `on_failure?: Vec<StepRef>`, `on_return?: HashMap<String, Vec<StepRef>>`, `then: Vec<StepRef>`, `meta: StepMeta`
- `StepRef` — `Id(String)` or `Inline(Box<Step>)`
- `Action` — tagged enum on `kind`:
  - `Shell { commands: Vec<String>, dir?, env? }` (commands accepts single string or array)
  - `Git { repo, dest, on_conflict }`
  - `Fs { op: FsOp, if_exists?, if_not_exists? }`
  - `Io { op: IoOp }` (`IoOp::Write { level, message, markup }` or `IoOp::Read { read, prompt?, default?, secret }`)
  - `Var { name, source: VarSource }`
  - `Cond { cmp: String, when: HashMap<String, Vec<StepRef>>, default?: Vec<StepRef> }`
  - `Rig { file: String, set: Option<HashMap<String, String>> }`
- `VarSource` — `Command { command }`, `From { from: StepRef }`, `To { to: StepRef }`, `File { file: String }`
- `FsOp` — enum; every variant carries `expand: ExpandFlags`:
  - `Create { path: Vec<String>, recurse, content?, expand }` (append supported with content)
  - `Symlink { from, to, expand }`
  - `Copy { from, to, expand }` (append supported: src appended to dst)
  - `Move { from, to, expand }`
  - `Delete { path: Vec<String>, recurse, expand }`
- `ExpandFlags` — `{ from, to, path, contents }`; consts `ALL`, `NONE`, `PATHS` (default). Custom deserialize: `true`/`false` shorthand or object form with PATHS-defaulted missing fields.
- `Condition` — `Action(skip/overwrite/append/panic)` or `Execute { execute: StepRef }`
- `GitOnConflict` — `Skip` (default), `Pull`, `Fail`
- `StepMeta` — `optional`, `fallible`, `sudo`, `silent: Vec<Silent>`, `retries?`, `retry_delay?`, `shell?: ShellConfig`
- `ShellConfig` — `{ cmd: String, args: Vec<String> }`; custom deserialize: string shorthand (`"bash"` → `{cmd:"bash", args:["-c"]}`) or object. Platform default: `sh -c` (Unix), `cmd /C` (Windows).
- `BundleMeta` (in `bundle.rs`) — `extract_to: ExtractTo`, `cleanup: Cleanup`, `binary: Vec<String>` (globset patterns)
- `ExtractTo` — `Named(Tmp | Cwd | Home)` or `Custom { path: String }`; default `Named(Tmp)`
- `Cleanup` — `Always`, `OnSuccess` (default), `Never`
- `BundleCtx` (runtime) — `root: PathBuf`, `binary: BinaryMatcher` (globset wrapper), `cleanup`, `succeeded: AtomicBool`, `_temp_dir: Option<tempfile::TempDir>`. `Drop` impl honors the cleanup policy.
- `Scope` (in `vars.rs`) — runtime variable store with dot-path keys; resolves `#` built-ins (`#timestamp`, `#now`, `#pwd`, `#bundle`) and `@`-mutables. `bundle_root: Option<String>` populated via `set_bundle_root`.
- `VarRef` (in `vars.rs`) — parsed `{{...}}` expression: `prefix` (#/@/none), `path` (segments), optional `format`.

## Variable System

- `#NAME` — built-in: `#timestamp` (startup), `#now` (eval-time), `#pwd` (startup), `#os` (linux/macos/windows), `#arch` (x86_64/aarch64/etc.)
- `@NAME` — runtime-mutable, not CLI-settable
- `@name` — runtime-mutable AND CLI-settable via `--set`
- `NAME`  — immutable constant from `meta.vars`
- `name`  — set from `meta.vars` or `--set`, immutable after startup
- Category is determined by the **first character of the first path segment** (uppercase vs lowercase).
- Nested vars: `{{super.mario.bros}}` flattens `meta.vars` to dot-path keys.
- Runtime substitution happens in executor before each action executes. `{{...}}` are preserved in parsed config until then.

## Executor Rules

- `shell`: run each command via configured shell (default `sh -c` / `cmd /C`), apply dir/env, return exit code
- `git`: clone if missing; on-conflict controls skip/pull/fail
- `fs`: dispatch to create/symlink/copy/move/delete with condition handling; create supports `content` for inline file writing
- `io`: print message with level prefix; write to log file if configured
- `var`: set `@` variable from command stdout / step stdout / file contents / (stdin feed via `to`)
- `cond`: substitute `cmp`, match against `when` keys, run matched step ref(s); fall back to `default` if no match
- `rig`: substitute `file`, parse sub-config, build scope with parent vars + `set` overrides, run sub-config steps
- Handler resolution: `on-return[exact code]` → `on-return["_"]` → `on-success`/`on-failure`
- `then`: run sequentially after action succeeds or is handled; skipped if action fails unhandled
- `optional`: skipped in normal flow; only runs when referenced
- `fallible`: failure logged, doesn't halt run, `then` skipped
- `retries`: auto-retry on failure N times; on-failure only invoked after all retries exhausted
- `retry-delay`: sleep before each retry (not on first run)
- Cycle protection: hard limit of 64 entries per step (not user-configurable)
- `silent`: suppresses stdout/stderr; `--verbose` overrides
- `dry_run`: full audit showing all steps, meta, conditions, handlers, then, summary
- `--only <id>`: resolve `depends-on` transitively, then run the target step
- `--parallel` / `meta.parallel`: run steps in topological order with concurrent execution at each DAG level
- `--validate`: parse-only validation
- Runtime substitution: all string fields in actions (`commands`, `dir`, `env`, `repo`, `dest`, `path`, `from`, `to`, `content`, `message`) are substituted via `Scope` before use.

## Coding Practices

- No `.unwrap()` in non-test code — use `.expect("reason")` or propagate with `?`
- `cargo clippy -- -D warnings` must pass
- `cargo fmt` must pass
- Update docs (README, CONTEXT, AGENTS, schema) alongside code changes
- Pre-commit hook enforces fmt + clippy; CI additionally runs tests on Linux, Windows, macOS

## Validation (at parse time)

- Duplicate step IDs rejected
- Unknown ID references rejected (then, on-success, on-failure, on-return, if-exists/if-not-exists execute)
- Undefined `NAME` / `name` vars (not in `meta.vars` or `--set`) rejected
- `@` vars need no definition (provided at runtime)
- `var` action targeting a non-mutable var (no `@` prefix) rejected
- Invalid aml markup in `io` with `markup: true` rejected
- `depends-on` cycles rejected at parse time (DFS)
- Cycles allowed (enforced at runtime via hard entry limit)
