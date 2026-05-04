# Agent Instructions for rig

Read `CONTEXT.md` first for full project context.

## Current State

- Fully implemented with new schema
- 51 tests passing, clippy clean
- JSONC support via `json_comments` crate

## File Structure

```
schema.json       — JSON Schema (draft-07) for editor validation/autocompletion
setup.json        — Sample config referencing schema.json
examples/
  dev-env.jsonc   — Annotated example with all features
  bootstrap.jsonc — Example: install Rust, clone repo, build rig
src/
  main.rs         — CLI entry point (clap derive) + orchestration
  config.rs       — Config/Step/Action types + parse_config() + step index + validation
  executor.rs     — Runner with execute dispatch, dry-run audit, retry tracking
  inspect.rs      — --list and --describe display logic
  path.rs         — tilde expansion helper
  style.rs        — aml wrapper for colored output
```

## Data Model

- `Config` — `name`, `version`, optional `description`, `meta: Meta`, `steps: Vec<Step>`
- `Meta` — `log?`, `silent: Vec<Silent>`, `sudo`, `retries?`, `retry_delay?` (top-level defaults)
- `Step` — `id?`, `name`, `description?`, `action: Action`, `on_success?: Vec<StepRef>`, `on_failure?: Vec<StepRef>`, `on_return?: HashMap<String, Vec<StepRef>>`, `then: Vec<StepRef>`, `meta: StepMeta`
- `StepRef` — `Id(String)` or `Inline(Box<Step>)` (unified, was StepRef + ChildRef)
- `Action` — tagged enum on `kind`:
  - `Shell { commands: Vec<String>, dir?, env? }` (commands accepts single string or array)
  - `Git { repo, dest, on_conflict }`
  - `Fs { op: FsOp, if_exists?, if_not_exists? }`
  - `Io { level: IoLevel, message, markup }`
- `FsOp` — enum:
  - `Create { path: Vec<String>, recurse, content? }` (append supported with content)
  - `Symlink { from, to }`
  - `Copy { from, to }` (append supported: src appended to dst)
  - `Move { from, to }`
  - `Delete { path: Vec<String>, recurse }`
- `Condition` — `Action(skip/overwrite/append/panic)` or `Execute { execute: StepRef }`
- `GitOnConflict` — `Skip` (default), `Pull`, `Fail`
- `StepMeta` — `optional`, `fallible`, `sudo`, `silent: Vec<Silent>`, `retries?`, `retry_delay?` (per-step overrides)

## Executor Rules

- `shell`: run each command via `sh -c`, apply dir/env, return exit code
- `git`: clone if missing; on-conflict controls skip/pull/fail
- `fs`: dispatch to create/symlink/copy/move/delete with condition handling; create supports `content` for inline file writing
- Handler resolution: `on-return[exact code]` → `on-return["_"]` → `on-success`/`on-failure`
- `then`: run sequentially after action succeeds or is handled; skipped if action fails unhandled
- `optional`: skipped in normal flow; only runs when referenced
- `fallible`: failure logged, doesn't halt run, `then` skipped
- `retries`: auto-retry on failure N times; on-failure only invoked after all retries exhausted
- `retry-delay`: sleep before each retry (not on first run)
- Cycle protection: hard limit of 64 entries per step (not user-configurable)
- `silent`: suppresses stdout/stderr; `--verbose` overrides
- `dry_run`: full audit showing all steps, meta, conditions, handlers, then, summary
- `--only <id>`: run/audit a single step
- `--validate`: parse-only validation

## Validation (at parse time)

- Duplicate step IDs rejected
- Unknown ID references rejected (then, on-success, on-failure, on-return, if-exists/if-not-exists execute)
- Cycles allowed (enforced at runtime via hard entry limit)
