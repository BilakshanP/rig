# devsetup — Project Context

## What is this?

A Rust CLI tool that reads a JSON/JSONC config and executes setup steps to bootstrap dev environments. Declarative, with dry-run support.

## Action Kinds

| Kind    | Description                                      |
|---------|--------------------------------------------------|
| `shell` | Run commands via `sh -c` with optional dir/env   |
| `git`   | Clone a repo; handle existing dest               |
| `fs`    | File operations: create, symlink, copy, move, delete |

## Config Structure

Actions are nested objects with a `kind` discriminator. Steps can have an `id` for referencing, `children` that run after the action, and `meta` for execution control.

```json
{
  "name": "my-env",
  "version": "1.0.0",
  "max-retries": 2,
  "steps": [
    {
      "id": "install",
      "name": "Install tools",
      "action": {
        "kind": "shell",
        "commands": ["apt install -y ripgrep"],
        "dir": "~",
        "env": { "DEBIAN_FRONTEND": "noninteractive" },
        "on-return": { "1": "handle-error", "_": "handle-error" }
      },
      "children": ["next-step"],
      "meta": {
        "fallible": true,
        "silent": ["stdout"],
        "max-retries": 3,
        "retry-delay": 2.0
      }
    }
  ]
}
```

## Key Features

- **`on-return`** — map exit codes to step refs (by ID or inline). `_` = wildcard. Handled steps still run children.
- **`if-exists` / `if-not-exists`** — `skip`, `overwrite`, `append`, `panic`, or `{ "execute": "step-id" }`
- **`children`** — run after parent succeeds. Array of step IDs or inline steps. Skipped if parent fails.
- **`meta.optional`** — skipped in normal flow; only runs when referenced by ID.
- **`meta.fallible`** — failure logged but doesn't halt the run. Children don't run on failure.
- **`meta.silent`** — suppress `stdout`/`stderr`; shown with `--verbose`.
- **`meta.max-retries`** — how many times a step can be re-entered via references. Overrides global.
- **`meta.retry-delay`** — seconds to sleep before each retry.
- **`max-retries` (top-level)** — global default for all steps. Steps without per-step or global max-retries cannot be re-entered (cycles caught at runtime).
- **Tilde expansion** — `~` expands to `$HOME` in all path fields.
- **JSONC support** — `//` and `/* */` comments via `json_comments` crate.
- **Validation** — duplicate IDs and unknown step references caught at parse time.

## FS Actions

| Action    | Fields needed     | Supports                     |
|-----------|-------------------|------------------------------|
| `create`  | `path`            | `recurse`, `if-exists`       |
| `symlink` | `from`, `to`      | `if-exists`                  |
| `copy`    | `from`, `to`      | `if-exists`, `if-not-exists` |
| `move`    | `from`, `to`      | `if-exists`, `if-not-exists` |
| `delete`  | `path`            | `recurse`, `if-not-exists`   |

`path` can be a string or array. Trailing `/` = directory.

## CLI

```
devsetup <config-file>                # Run
devsetup <config-file> --dry-run      # Full audit: shows all steps, meta, conditions, children
devsetup <config-file> --verbose      # Show suppressed output
devsetup <config-file> --only <id>    # Run a single step by ID
devsetup <config-file> --validate     # Parse and validate without executing
```

## Dry-Run Audit

`--dry-run` shows a complete audit of the config including:
- All steps (including optional), with IDs and meta flags
- Action details (commands, paths, env, dir)
- `on-return` code→step mappings
- `if-exists`/`if-not-exists` conditions
- Children (ID refs and inline)
- Summary (total, optional, fallible counts)

## JSON Schema

`schema.json` (draft-07) at project root. Reference via `"$schema": "./schema.json"`.

## Tech Stack

- Rust, serde + serde_json, clap (derive), json_comments
- Single binary, no runtime deps
