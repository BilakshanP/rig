# rig

A declarative CLI tool for bootstrapping dev environments from JSON configs.

## Install

```bash
cargo install --git https://github.com/BilakshanP/rig.git
```

Or from a local clone:

```bash
cargo install --path .
```

## Usage

```bash
rig setup.json              # Run the setup
rig setup.json --dry-run    # Full audit — see everything before executing
rig setup.json --validate   # Parse and validate only
rig setup.json --only <id>  # Run a single step by ID
rig setup.json --verbose    # Show suppressed output
rig setup.json --list       # One-line summary of all steps
rig setup.json --describe <id>          # Describe a step in detail
rig setup.json --describe <id> --depth  # Expand sub-steps recursively
rig setup.json --describe <id> --depth 2  # Expand up to 2 levels
```

Configs can also be loaded from URLs:

```bash
rig https://example.com/setup.jsonc --set project=my-app
```

## Config Format

Configs are JSON or JSONC (comments supported). A config has a name, version, optional top-level meta, and a list of steps:

```jsonc
{
  "$schema": "./schema.json",
  "name": "my-env",
  "version": "1.0.0",
  "meta": {
    "retries": 2,                    // global default: retry failed steps twice
    "log": "~/logs/{{name}}-{{timestamp}}.log"  // save run transcript
  },
  "steps": [
    {
      "id": "install-tools",
      "name": "Install dev tools",
      "action": {
        "kind": "shell",
        "commands": ["apt install -y ripgrep fzf"],
        "dir": "~",
        "env": { "DEBIAN_FRONTEND": "noninteractive" }
      }
    }
  ]
}
```

## Action Kinds

### shell

Run commands via `sh -c`. Supports working directory and env vars.

```jsonc
{
  "kind": "shell",
  "commands": ["echo hello", "make build"],
  "dir": "~/project",
  "env": { "CC": "gcc" }
}
```

### git

Clone a repo. Handle existing destinations with `on-conflict`.

```jsonc
{
  "kind": "git",
  "repo": "https://github.com/user/dotfiles.git",
  "dest": "~/.dotfiles",
  "on-conflict": "pull" // skip (default), pull, fail
}
```

### fs

File system operations use nested sub-action objects:

```jsonc
// Create directories and files (trailing / = directory)
{ "kind": "fs", "create": { "path": ["~/projects/", "~/tmp/"], "recurse": true }, "if-exists": "skip" }

// Create a file with inline content
{ "kind": "fs", "create": { "path": "~/.config/app.json", "content": "{\"theme\": \"dark\"}", "recurse": true } }

// Symlink
{ "kind": "fs", "symlink": { "from": "~/.dotfiles/.bashrc", "to": "~/.bashrc" }, "if-exists": "overwrite" }

// Copy / Move
{ "kind": "fs", "copy": { "from": "template.conf", "to": "~/.config/app.conf" }, "if-not-exists": "panic" }

// Delete
{ "kind": "fs", "delete": { "path": "~/.cache/old", "recurse": true }, "if-not-exists": "skip" }
```

### io

Structured logging. Messages are plain text by default; set `markup: true` to parse as aml.

```jsonc
{ "kind": "io", "level": "info", "message": "Starting setup..." }
{ "kind": "io", "level": "warn", "message": "Config already exists" }
{ "kind": "io", "level": "error", "message": "Missing dependency" }
{ "kind": "io", "level": "info", "message": "<fg>Done!</f>", "markup": true }
```

Levels: `log`, `info`, `warn`, `error`. Write-mode io always succeeds and never affects step execution.

Read-mode io prompts for input and stores it in a runtime-mutable `@var`:

```jsonc
{ "kind": "io", "read": "@env", "prompt": "Environment: ", "default": "dev" }
{ "kind": "io", "read": "@password", "prompt": "Password: ", "secret": true }
```

- `read` must be an `@`-prefixed var (runtime-mutable)
- `prompt` is optional
- `default` is used if the user enters an empty line; without one, the var stays unset (later references render as `{{@env}}` in yellow)
- `secret: true` masks each keystroke with `*` as you type, and shows the captured value as `****` afterward
- In `--dry-run` the `default` is used if set; otherwise the var is left unset

## Step Features

### Handlers: on-success, on-failure, on-return

Steps can react to their action's outcome. Resolution order: `on-return[exact code]` → `on-return["_"]` → `on-success`/`on-failure`.

```jsonc
{
  "name": "Install tools",
  "action": { "kind": "shell", "commands": ["apt install -y ripgrep"] },
  "on-success": "next-step",           // exit 0
  "on-failure": "error-handler",       // non-zero exit (after retries exhausted)
  "on-return": {
    "1": "retry-step",                 // overrides on-failure for exit code 1
    "_": "catch-all"                   // overrides on-failure for all other codes
  }
}
```

Handler values can be a single step ref or an array:

```jsonc
"on-success": ["step-a", "step-b"]
```

### then

Steps can have sub-steps that run after the action succeeds (or is handled). Reference by ID or inline:

```jsonc
{
  "name": "Setup",
  "action": { "kind": "shell", "commands": ["make install"] },
  "then": [
    "verify-step",
    { "name": "inline cleanup", "action": { "kind": "shell", "commands": ["make clean"] } }
  ]
}
```

### Meta

Control execution behavior per step:

```jsonc
"meta": {
  "optional": true,     // Skipped unless referenced by ID
  "fallible": true,     // Failure doesn't halt the run
  "sudo": true,         // Run shell commands with sudo
  "silent": ["stdout"], // Suppress output (--verbose overrides)
  "retries": 3,         // Auto-retry on failure
  "retry-delay": 2.0    // Seconds to wait before each retry
}
```

### Conditions

`if-exists` and `if-not-exists` accept: `"skip"`, `"overwrite"`, `"append"`, `"panic"`, or execute a step:

```jsonc
"if-exists": { "execute": "backup-handler" }
```

`"append"` is supported for:
- `fs create` with `content` — appends content to the existing file
- `fs copy` — appends source file content to the existing destination

For `symlink`/`move`, `append` is not meaningful and will error.

## Dry Run

`--dry-run` shows a complete audit of the config — all steps including optional ones, IDs, meta flags, conditions, handlers, and sub-steps:

```
[dry-run] my-env
(3 steps, 1 optional, 0 fallible)

→ Install tools (id: install) [silent: stdout]
    sh -c "apt install -y ripgrep"
    on-success: next-step
    on-failure: error-handler

→ Error handler (id: error-handler) [optional] [fallible]
    sh -c "echo 'something went wrong'"

Summary: 3 steps (1 optional, 0 fallible)
```

## Schema

A JSON Schema (`schema.json`) is included for editor autocompletion and validation. Reference it in your config:

```json
{ "$schema": "./schema.json" }
```

## Variables

Rig has a layered variable system with scopes and mutability. Variables use `{{name}}` syntax, are resolved at runtime (not parse time), and fall into five categories based on prefix and case:

| Syntax | Where set | Mutable at runtime |
|--------|-----------|--------------------|
| `{{#NAME}}` | Built-in (system-provided) | No |
| `{{@NAME}}` | `meta.vars` + `var` action | Yes |
| `{{@name}}` | `meta.vars`, `--set`, `var` action | Yes |
| `{{NAME}}` | `meta.vars` | No (constant) |
| `{{name}}` | `meta.vars` or `--set` | No (set once at startup) |

**Case rule:** the first character of the first path segment determines upper/lower category.

### Built-in variables

```jsonc
"{{#timestamp}}"         // %Y%m%dT%H%M%S at startup (e.g., 20260504T152259)
"{{#timestamp:%Y-%m-%d}}" // Custom strftime format
"{{#now}}"               // Current time, evaluated each use
"{{#now:%H:%M:%S}}"      // Custom strftime at each use
"{{#pwd}}"               // Current working directory at startup
```

### Defaults via `meta.vars`

`meta.vars` provides default values. CLI `--set key=value` overrides them. Values are literal strings — no substitution inside them.

```jsonc
{
  "meta": {
    "vars": {
      "project": "my-app",
      "env": "dev",
      "super": {
        "mario": { "bros": "smb" }   // nested: access as {{super.mario.bros}}
      }
    }
  },
  "steps": [...]
}
```

```bash
rig setup.json                    # uses defaults
rig setup.json --set env=prod     # overrides env
```

### Mutable variables and the `var` action

`@`-prefixed variables are runtime-mutable via the `var` action:

```jsonc
// Capture a shell command's stdout
{ "kind": "var", "name": "@hash", "command": "git rev-parse HEAD" }

// Run a step and capture its stdout
{ "kind": "var", "name": "@build_dir", "from": "make-tempdir-step" }

// Feed a variable's value as stdin to a step
{ "kind": "var", "name": "@config", "to": "apply-config-step" }

// Read a file's contents
{ "kind": "var", "name": "@config_data", "file": "~/config.json" }
```

Only `@`-prefixed variables are writable at runtime. Writing to `{{NAME}}` or `{{name}}` is a parse-time error.

### Listing variables

Use `--vars` to list all variables referenced in a config with their defaults:

```bash
rig setup.json --vars
```

Undefined non-`@` variables are rejected at parse time. Use `\{\{` to escape literal double braces.

## Examples

See [`examples/`](examples/) for sample configs.
