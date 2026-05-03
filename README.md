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

Configs are JSON or JSONC (comments supported). A config has a name, version, and a list of steps:

```jsonc
{
  "$schema": "./schema.json",
  "name": "my-env",
  "version": "1.0.0",
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

Run commands via `sh -c`. Supports working directory, env vars, and exit code handling.

```jsonc
{
  "kind": "shell",
  "commands": ["echo hello", "make build"],
  "dir": "~/project",
  "env": { "CC": "gcc" },
  "on-return": {
    "0": "next-step",    // exit 0 → run step by ID
    "_": "error-handler" // wildcard for unmatched codes
  }
}
```

### git

Clone a repo. Handle existing destinations.

```jsonc
{
  "kind": "git",
  "repo": "https://github.com/user/dotfiles.git",
  "dest": "~/.dotfiles",
  "if-exists": "pull" // skip (default), pull, fail
}
```

### fs

File system operations: `create`, `symlink`, `copy`, `move`, `delete`.

```jsonc
// Create directories and files (trailing / = directory)
{ "kind": "fs", "action": "create", "path": ["~/projects/", "~/tmp/"], "recurse": true, "if-exists": "skip" }

// Symlink
{ "kind": "fs", "action": "symlink", "from": "~/.dotfiles/.bashrc", "to": "~/.bashrc", "if-exists": "overwrite" }

// Copy / Move
{ "kind": "fs", "action": "copy", "from": "template.conf", "to": "~/.config/app.conf", "if-not-exists": "panic" }

// Delete
{ "kind": "fs", "action": "delete", "path": "~/.cache/old", "recurse": true, "if-not-exists": "skip" }
```

## Step Features

### Children

Steps can have children that run after the parent succeeds. Reference by ID or inline:

```jsonc
{
  "name": "Setup",
  "action": { "kind": "shell", "commands": ["make install"] },
  "children": [
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
  "silent": ["stdout"], // Suppress output (--verbose overrides)
  "max-retries": 3,     // Allow re-entry via references
  "retry-delay": 2.0    // Seconds to wait before retry
}
```

### Conditions

`if-exists` and `if-not-exists` accept: `"skip"`, `"overwrite"`, `"append"`, `"panic"`, or execute a step:

```jsonc
"if-exists": { "execute": "backup-handler" }
```

## Dry Run

`--dry-run` shows a complete audit of the config — all steps including optional ones, IDs, meta flags, conditions, on-return mappings, and children:

```
[dry-run] my-env
(3 steps, 1 optional, 0 fallible)

→ Install tools (id: install) [silent: stdout]
    sh -c "apt install -y ripgrep"
    on-return:
      0 → next-step
      _ → error-handler

→ Error handler (id: error-handler) [optional] [fallible]
    sh -c "echo 'something went wrong'"

Summary: 3 steps (1 optional, 0 fallible)
```

## Schema

A JSON Schema (`schema.json`) is included for editor autocompletion and validation. Reference it in your config:

```json
{ "$schema": "./schema.json" }
```

## Examples

See [`examples/`](examples/) for sample configs.
