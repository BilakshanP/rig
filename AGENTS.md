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
  path.rs         — tilde expansion helper
  style.rs        — aml wrapper for colored output
```

## Data Model

- `Config` — `name`, `version`, optional `description`, optional `retries`, `steps: Vec<Step>`
- `Step` — `id?`, `name`, `description?`, `action: Action`, `on_success?`, `on_failure?`, `on_return?`, `then: Vec<ChildRef>`, `meta: Meta`
- `ChildRef` — `Id(String)` or `Inline(Box<Step>)`
- `StepRefs` — `Single(StepRef)` or `Multiple(Vec<StepRef>)` (used for on-success/on-failure/on-return values)
- `StepRef` — `Id(String)` or `Inline(Box<Step>)`
- `Action` — tagged enum on `kind`:
  - `Shell { commands, dir?, env? }`
  - `Git { repo, dest, on_conflict }`
  - `Fs { op: FsOp, if_exists?, if_not_exists? }`
- `FsOp` — enum:
  - `Create { path: PathSpec, recurse, content? }`
  - `Symlink { from, to }`
  - `Copy { from, to }`
  - `Move { from, to }`
  - `Delete { path: PathSpec, recurse }`
- `PathSpec` — `Single(String)` or `Multiple(Vec<String>)`
- `Condition` — `Action(skip/overwrite/append/panic)` or `Execute { execute: StepRef }`
- `GitOnConflict` — `Skip` (default), `Pull`, `Fail`
- `Meta` — `optional`, `fallible`, `sudo`, `silent: Vec<Silent>`, `retries?`, `retry_delay?`

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
