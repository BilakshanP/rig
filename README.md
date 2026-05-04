# rig

A declarative CLI tool for bootstrapping dev environments from JSON configs.

## Install

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
{ "kind": "io", "level": "info", "message": "<fg>✓ Done!</f>", "markup": true }
```

Levels: `log`, `info`, `warn`, `error`. IO actions always succeed and never affect step execution.

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

Use `--set key=value` to inject variables into configs. Variables use `{{key}}` syntax and are substituted before parsing:

```jsonc
{
  "name": "setup-{{project}}",
  "steps": [{
    "name": "Create project",
    "action": {
      "kind": "shell",
      "commands": ["mkdir -p ~/projects/{{project}}"]
    }
  }]
}
```

```bash
rig setup.json --set project=my-app --set env=prod
```

Undefined variables fail immediately at startup. Use `\{\{` to escape literal double braces.

### Built-in Variables

`{{timestamp}}` is substituted at startup with the current time in `%Y%m%dT%H%M%S` format (e.g., `20260504T152259`). Use a custom strftime format with `{{timestamp:FORMAT}}`:

```jsonc
"log": "~/logs/{{timestamp}}.log"              // 20260504T152259.log
"log": "~/logs/{{timestamp:%Y-%m-%d}}.log"     // 2026-05-04.log
"log": "~/logs/{{timestamp:%H-%M-%S}}.log"     // 15-22-59.log
```

## Examples

See [`examples/`](examples/) for sample configs.
