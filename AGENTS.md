# Agent Instructions for rig

Read `CONTEXT.md` first for full project context.

## Current State

- Fully implemented with new schema
- 39 tests passing, clippy clean
- JSONC support via `json_comments` crate

## File Structure

```
schema.json       — JSON Schema (draft-07) for editor validation/autocompletion
setup.json        — Sample config referencing schema.json
example.jsonc     — Annotated example with all features
examples/
  bootstrap.jsonc — Example: install Rust, clone repo, build devsetup
src/
  main.rs         — CLI entry point (clap derive) + orchestration
  config.rs       — Config/Step/Action types + parse_config() + step index + validation
  executor.rs     — Runner with execute dispatch, dry-run audit, retry tracking
  path.rs         — tilde expansion helper
  style.rs        — aml wrapper for colored output
```

## Data Model

- `Config` — `name`, `version`, optional `description`, optional `max-retries`, `steps: Vec<Step>`
- `Step` — `id?`, `name`, `description?`, `action: Action`, `children: Vec<ChildRef>`, `meta: Meta`
- `ChildRef` — `Id(String)` or `Inline(Box<Step>)`
- `Action` — tagged enum on `kind`:
  - `Shell { commands, dir?, env?, on_return? }`
  - `Git { repo, dest, if_exists }`
  - `Fs { action: FsAction, recurse, target: FsTarget, if_exists?, if_not_exists? }`
- `FsAction` — `Create`, `Symlink`, `Copy`, `Move`, `Delete`
- `FsTarget` — `FromTo { from, to }` or `Path { path: PathSpec }`
- `PathSpec` — `Single(String)` or `Multiple(Vec<String>)`
- `Condition` — `Action(skip/overwrite/append/panic)` or `Execute { execute: StepRef }`
- `StepRef` — `Id(String)` or `Inline(Box<Step>)`
- `Meta` — `optional`, `fallible`, `silent: Vec<Silent>`, `max_retries?`, `retry_delay?`

## Executor Rules

- `shell`: run each command via `sh -c`, apply dir/env, check on-return map
- `git`: clone if missing; if-exists controls skip/pull/fail
- `fs`: dispatch to create/symlink/copy/move/delete with condition handling
- `children`: run sequentially after parent succeeds; skipped if parent fails (even if fallible)
- `optional`: skipped in normal flow; only runs when referenced
- `fallible`: failure logged, doesn't halt run, children skipped
- `on-return`: exit code → step ref lookup; `_` wildcard; handled steps still run children
- `max-retries`: per-step overrides global; no config = step can only be entered once
- `retry-delay`: sleep before re-entry (not on first run)
- `silent`: suppresses stdout/stderr; `--verbose` overrides
- `dry_run`: full audit showing all steps, meta, conditions, children, summary
- `--only <id>`: run/audit a single step
- `--validate`: parse-only validation

## Validation (at parse time)

- Duplicate step IDs rejected
- Unknown ID references rejected (children, on-return, if-exists/if-not-exists execute)
- Cycles allowed (enforced at runtime via max-retries)
