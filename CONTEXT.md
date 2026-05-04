# rig — Project Context

## What is this?

A Rust CLI tool called `rig` that reads a JSON/JSONC config and executes setup steps to bootstrap dev environments. Declarative, with dry-run support and colored output.

## Action Kinds

| Kind    | Description                                      |
|---------|--------------------------------------------------|
| `shell` | Run commands via `sh -c` with optional dir/env   |
| `git`   | Clone a repo; handle existing dest               |
| `fs`    | File operations: create, symlink, copy, move, delete |

## Config Structure

Actions are nested objects with a `kind` discriminator. Steps can have an `id` for referencing, `then` sub-steps that run after the action, and `meta` for execution control.

```json
{
  "name": "my-env",
  "version": "1.0.0",
  "retries": 2,
  "steps": [
    {
      "id": "install",
      "name": "Install tools",
      "action": {
        "kind": "shell",
        "commands": ["apt install -y ripgrep"],
        "dir": "~",
        "env": { "DEBIAN_FRONTEND": "noninteractive" }
      },
      "on-success": "next-step",
      "on-failure": "handle-error",
      "on-return": { "1": "handle-error", "_": "handle-error" },
      "then": ["next-step"],
      "meta": {
        "fallible": true,
        "silent": ["stdout"],
        "retries": 3,
        "retry-delay": 2.0
      }
    }
  ]
}
```

## Key Features

- **`on-success`** — step ref(s) to run on exit 0 (unless overridden by on-return).
- **`on-failure`** — step ref(s) to run on non-zero exit (after retries exhausted, unless overridden by on-return).
- **`on-return`** — map exit codes to step refs. `_` = wildcard. Resolution: exact code → `_` → on-success/on-failure.
- **`then`** — sub-steps that run after the action succeeds or is handled. Array of step IDs or inline steps.
- **`if-exists` / `if-not-exists`** — `skip`, `overwrite`, `append`, `panic`, or `{ "execute": "step-id" }` (fs actions only).
- **`on-conflict`** — git-specific: `skip` (default), `pull`, `fail`.
- **`meta.optional`** — skipped in normal flow; only runs when referenced by ID.
- **`meta.fallible`** — failure logged but doesn't halt the run. `then` steps don't run on failure.
- **`meta.sudo`** — run shell commands with `sudo`. Pre-flight `sudo -v` runs at startup if any step needs it.
- **`meta.silent`** — suppress `stdout`/`stderr`; shown with `--verbose`.
- **`meta.retries`** — auto-retry on failure N times. Overrides global `retries`.
- **`meta.retry-delay`** — seconds to sleep before each retry.
- **`retries` (top-level)** — global default retry count for all steps.
- **Cycle protection** — hard limit of 64 entries per step (not user-configurable).
- **Tilde expansion** — `~` expands to `$HOME` in all path fields.
- **JSONC support** — `//` and `/* */` comments via `json_comments` crate.
- **Colored output** — via `aml` crate (green success, yellow warnings, red errors, cyan IDs, dim labels).
- **Validation** — duplicate IDs and unknown step references caught at parse time.

## FS Actions

FS actions use nested sub-action objects within `kind: "fs"`:

| Sub-action | Fields                    | Supports                     |
|------------|---------------------------|------------------------------|
| `create`   | `path`, `recurse`, `content` | `if-exists`               |
| `symlink`  | `from`, `to`              | `if-exists`                  |
| `copy`     | `from`, `to`              | `if-exists`, `if-not-exists` |
| `move`     | `from`, `to`              | `if-exists`, `if-not-exists` |
| `delete`   | `path`, `recurse`         | `if-not-exists`              |

`path` can be a string or array. Trailing `/` = directory. `content` writes inline text to a file.

## CLI

```
rig <config-file>                # Run
rig <config-file> --dry-run      # Full audit: shows all steps, meta, conditions, handlers
rig <config-file> --verbose      # Show suppressed output
rig <config-file> --only <id>    # Run a single step by ID
rig <config-file> --validate     # Parse and validate without executing
```

## Dry-Run Audit

`--dry-run` shows a complete audit of the config including:
- All steps (including optional), with IDs and meta flags
- Action details (commands, paths, env, dir)
- `on-success`, `on-failure`, `on-return` handlers
- `if-exists`/`if-not-exists` conditions
- `then` sub-steps (ID refs and inline)
- Summary (total, optional, fallible counts)

## JSON Schema

`schema.json` (draft-07) at project root. Reference via `"$schema": "./schema.json"`.

## Tech Stack

- Rust, serde + serde_json, clap (derive), json_comments, aml
- Single binary, no runtime deps
