# rig — Project Context

## What is this?

A Rust CLI tool called `rig` that reads a JSON/JSONC config and executes setup steps to bootstrap dev environments. Declarative, with dry-run support and colored output. Configs can also be packaged as `.rig` bundles — tar.gz archives that ship a manifest together with real template files so large configs don't need to inline every file body as an escaped JSON string.

## Action Kinds

| Kind    | Description                                      |
|---------|--------------------------------------------------|
| `shell` | Run commands via configurable shell (default: `sh -c` / `cmd /C`) |
| `git`   | Clone a repo; handle existing dest               |
| `fs`    | File operations: create, symlink, copy, move, delete |
| `io`    | Structured logging (write) or prompt-and-read from stdin into a `@var` |
| `var`   | Set a runtime-mutable `@var` from a step's stdout or a shell command |
| `cond`  | String-based conditional dispatch: compare a value against keys, run matching step(s) |

## Config Structure

Actions are nested objects with a `kind` discriminator. Steps can have an `id` for referencing, `then` sub-steps that run after the action, and `meta` for execution control.

```json
{
  "name": "my-env",
  "version": "1.0.0",
  "meta": {
    "retries": 2,
    "log": "~/logs/{{name}}-{{#timestamp}}.log",
    "silent": ["stdout"],
    "sudo": false,
    "vars": { "project": "my-app" }
  },
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
- **Top-level `meta`** — global defaults for `retries`, `retry-delay`, `silent`, `sudo`, and `log` (run transcript path).
- **`io` action** — write: `level`/`message`/`markup`; read: `read`/`prompt?`/`default?`/`secret?` (stores line from stdin into an `@var`).
- **`var` action** — set a mutable `@var` from `command` (shell output), `from` (step stdout), `to` (feed variable to step stdin), or `file` (read file contents).
- **`cond` action** — string-based conditional dispatch: `cmp` is substituted at runtime, matched against `when` keys to run step ref(s); `default` is the fallback. Pairs with `io` read-mode for user-input branching.
- **Variable system** — 5 categories by prefix/case:
  - `#NAME` (built-in: `#timestamp`, `#now`, `#pwd`, `#bundle` (bundle runs only))
  - `@NAME` (mutable, runtime-only)
  - `@name` (mutable, CLI-settable)
  - `NAME` (constant from `meta.vars`)
  - `name` (from `meta.vars` or `--set`, immutable after startup)
- **Runtime substitution** — all `{{...}}` references resolved per-action, not at parse time.
- **Escape syntax** — `{{{{foo}}}}` renders to the literal string `{{foo}}`. Used by bundles to reference their own templated directory names without resolving them.
- **Nested vars** — `meta.vars` can contain nested objects; accessed via dot notation (e.g., `{{super.mario.bros}}`).
- **`meta.vars`** — literal default values for variables. CLI `--set key=value` overrides. Use `--vars` to list all variables referenced in a config with their defaults.
- **Markup validation** — io actions with `markup: true` are validated at parse time; invalid aml fails `--validate`.
- **Cycle protection** — hard limit of 64 entries per step (not user-configurable).
- **Tilde expansion** — `~` expands to home directory via `dirs` crate (cross-platform).
- **Configurable shell** — `meta.shell` (global) and per-step `meta.shell` override the default shell. String shorthand (`"bash"`) or object `{ "cmd": "...", "args": [...] }`. Defaults to `sh -c` on Unix, `cmd /C` on Windows.
- **Windows compatibility** — symlinks use platform APIs (clear error on permission failure), sudo is skipped with a warning, shell defaults to `cmd /C`.
- **JSONC support** — `//` and `/* */` comments via `json_comments` crate.
- **Colored output** — via `aml` crate (green success, yellow warnings, red errors, cyan IDs, dim labels).
- **Validation** — duplicate IDs and unknown step references caught at parse time.

## FS Actions

FS actions use nested sub-action objects within `kind: "fs"`:

| Sub-action | Fields                             | Supports                     |
|------------|------------------------------------|------------------------------|
| `create`   | `path`, `recurse`, `content`, `expand` | `if-exists`              |
| `symlink`  | `from`, `to`, `expand`             | `if-exists`                  |
| `copy`     | `from`, `to`, `expand`             | `if-exists`, `if-not-exists` |
| `move`     | `from`, `to`, `expand`             | `if-exists`, `if-not-exists` |
| `delete`   | `path`, `recurse`, `expand`        | `if-not-exists`              |

`path` can be a string or array. Trailing `/` = directory. `content` writes inline text to a file.

`expand` controls `{{var}}` substitution per field: `true` (all), `false`
(byte-exact, e.g., when a path literally contains `{{name}}`), or object
`{ "from": bool, "to": bool, "path": bool, "contents": bool }`. Default: paths
rendered, contents byte-exact. Inside a `.rig` bundle, `fs.copy` auto-renders
templated file contents when the source lives inside the bundle's staging
root (unless matched by `bundle.binary` globs).

## CLI

```
rig <config-file>                # Run
rig <config-file> --dry-run      # Full audit: shows all steps, meta, conditions, handlers
rig <config-file> --verbose      # Show suppressed output
rig <config-file> --only <id>    # Run a single step by ID
rig <config-file> --validate     # Parse and validate without executing
rig <config-file> --list         # One-line summary of all steps
rig <config-file> --describe <id>          # Describe a step in detail
rig <config-file> --describe <id> --depth  # Expand sub-steps recursively
rig <config-file> --describe <id> --depth 2  # Expand up to 2 levels

rig pack <dir> -o <file>.rig    # Build a .rig bundle from a directory
rig unpack <file>.rig -o <dir>  # Extract a .rig bundle
rig info <file>.rig             # Summary of manifest + file list
```

`<config-file>` can be a local path, a URL (`http://` / `https://`), a
`.rig` bundle archive, a git repo URL, or a local directory. Bundles are
auto-detected by `.rig` extension or gzip magic bytes. Git repo URLs
(GitHub, GitLab, Bitbucket, Codeberg, or any `.git` URL) are shallow-cloned
and treated as bundle source directories. SSH URLs (`git@host:user/repo.git`,
`ssh://...`) are also supported. Local directories containing
`manifest.json` or `manifest.jsonc` are run directly as bundles without
needing `rig pack`.

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

## Bundle Format

A `.rig` bundle is a tar.gz archive with a `manifest.json` or `manifest.jsonc`
at the root and arbitrary template files alongside it. The manifest is itself
a rig config with an optional `bundle` section:

```jsonc
{
  "name": "...",
  "bundle": {
    "extract-to": "tmp",           // "tmp" (default) | "cwd" | "home" | { "path": "..." }
    "cleanup": "on-success",       // "always" | "on-success" (default) | "never"
    "binary": ["assets/**/*.png"]  // globs for files copied byte-for-byte
  },
  "steps": [ ... ]
}
```

- `extract-to` chooses where the bundle is staged before the manifest runs.
- `cleanup` controls the staging dir's lifecycle:
  - `always` — remove unconditionally on exit.
  - `on-success` — remove only if the run succeeded; keep for inspection on failure.
  - `never` — keep and print the staging path.
- `binary` lists globs (relative to bundle root) whose files are copied raw; everything else goes through variable substitution when read via `fs.copy` from inside the bundle.

During a bundle run, `fs.copy` detects when the source path lives inside the
staging root and renders templated file contents on the way out. Sources
outside the staging root (or byte-matched by `binary`) are copied byte-for-byte.
`{{#bundle}}` resolves to the staging root so manifests can reference their
own payload files: `"from": "{{#bundle}}/{{{{name}}}}/pyproject.toml"` pairs
the `#bundle` substitution with a `{{{{name}}}}` escape to match the literal
on-disk directory named `{{name}}`.

## Tech Stack

- Rust, serde + serde_json, clap (derive), json_comments, chrono, aml
- Bundle I/O: tar + flate2, tempfile, globset
- Single binary, no runtime deps
