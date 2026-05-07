# rig

A powerful, cross-platform CLI tool for automating structured workflows from declarative JSON configs.

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
rig setup.json                            # Run a config
rig setup.rig                             # Run a bundle (see below)
rig setup.json --dry-run                  # Full audit — see everything before executing
rig setup.json --validate                 # Parse and validate only
rig setup.json --only <id>                # Run a single step by ID
rig setup.json --verbose                  # Show suppressed output
rig setup.json --parallel                 # Run steps concurrently (DAG-ordered)
rig setup.json --no-parallel              # Force sequential (overrides meta.parallel)
rig setup.json --list                     # One-line summary of all steps
rig setup.json --describe <id>            # Describe a step in detail
rig setup.json --describe <id> --depth    # Expand sub-steps recursively
rig setup.json --describe <id> --depth 2  # Expand up to 2 levels
rig setup.json --graph                    # Print the dependency graph (ASCII)
rig setup.json --dot                      # Print the dependency graph (DOT format)
rig setup.json --edges                    # Print the dependency graph (edge list)
rig setup.json --label                    # Include step names as labels in graph output
rig setup.json --placeholder              # Show unresolved variables as placeholders
```

### Output control

| Flags | Chrome | Command output | io messages | Errors |
|-------|--------|---------------|-------------|--------|
| (default) | ✓ | ✓ | ✓ | ✓ |
| `-s` | ✓ | ✗ | ✓ | ✓ |
| `-q` | ✗ | ✓ | ✓ | ✓ |
| `-q -s` | ✗ | ✗ | ✓ | ✓ |
| `-qq` | ✗ | ✗ | ✗ | ✓ |
| `-qqq` | ✗ | ✗ | ✗ | ✗ |

Configs and bundles can also be loaded from URLs, git repos, or local directories:

```bash
rig https://example.com/setup.jsonc --set project=my-app
rig https://example.com/setup.rig
rig https://github.com/user/dev-setup --set name=my-app  # clone repo, find manifest
rig git@github.com:user/dev-setup.git --set name=my-app  # SSH clone
rig ./my-template-dir                                    # local directory with manifest
rig https://github.com/user/templates.git --fragment rust # use a subdirectory
```

When given a git repo URL (GitHub, GitLab, Bitbucket, Codeberg, or any `.git`
URL), rig shallow-clones it and looks for `manifest.json` or `manifest.jsonc`
at the root — treating it as a bundle source directory. SSH URLs
(`git@host:user/repo.git`, `ssh://...`) are also supported. The same applies to
local directories: if the path is a directory containing a manifest, rig runs
it as a bundle without needing to `rig pack` first.

### Bundle subcommands

```bash
rig pack <dir>                  # Write to <dir>.rig in cwd
rig pack <dir> -o <file>.rig    # Explicit output path
rig unpack <file>.rig           # Extract to <file>/ (strip .rig)
rig unpack <file>.rig -o <dir>  # Explicit destination
rig info <file>.rig             # Show manifest summary + file list
```

## Config Format

Configs are JSON or JSONC (comments supported). A config has a name, version, optional top-level meta, and a list of steps:

```jsonc
{
  "$schema": "./schema.json",
  "name": "my-env",
  "version": "1.0.0",
  "meta": {
    "retries": 2,                                // global default: retry failed steps twice
    "log": "~/logs/{{name}}-{{#timestamp}}.log", // save run transcript
    "env": { "CI": "true" },                     // global env vars for all shell commands
    "parallel": true,                            // run steps concurrently when depends-on allows
    "parallel-output": true,                     // interleave step output in parallel mode
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

Run commands via a configurable shell. Defaults to `sh -c` on Unix, `cmd /C` on Windows. Supports working directory and env vars.

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

#### `expand` flag

Every fs sub-action accepts an optional `expand` flag that controls `{{var}}`
substitution per-field. The default (`{ "from": true, "to": true, "path": true, "contents": false }`)
renders `{{...}}` in path arguments — matching the default behavior of every
other action — while leaving file contents byte-exact. Shorthand `true` enables
every field including `contents`; `false` disables every field, useful when
a path contains literal `{{...}}` directory names (see the [python-project
bundle](examples/python-project/)).

```jsonc
// Paths rendered, contents rendered too (e.g. for templating a checked-in file)
{ "kind": "fs", "copy": { "from": "template.toml", "to": "~/.config/{{app}}.toml", "expand": true } }

// Byte-exact — use when a filename literally contains {{...}}
{ "kind": "fs", "copy": { "from": "./{{ph}}/data", "to": "~/data", "expand": false } }

// Per-field — render destination but match source as literal
{ "kind": "fs", "copy": { "from": "{{name}}/file", "to": "./{{name}}/file", "expand": { "from": false } } }
```

Inside a `.rig` bundle, `fs.copy` automatically renders templated file
contents when the source lives inside the bundle's staging root (bypass with
`bundle.binary` globs for raw files). The `expand: { contents: false }` flag
has no effect for bundle sources — use `bundle.binary` to opt out of rendering.

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

### cond

String-based conditional dispatch. Compares a substituted value against keys and runs the matching step(s):

```jsonc
{
  "kind": "cond",
  "cmp": "{{@env}}",
  "when": {
    "dev": "dev-setup",
    "prod": ["prod-deploy", "notify-team"]
  },
  "default": "fallback-step"
}
```

- `cmp` is substituted at runtime, then matched against `when` keys
- `when` values can be a single step ref (ID or inline) or an array
- `default` runs when no key matches (optional — if absent and no match, nothing happens)
- Pairs naturally with `io` read-mode to branch on user input

### rig

Execute another config file as a sub-config. Parent variables flow down; `set` overrides specific values:

```jsonc
{
  "kind": "rig",
  "file": "./platform/linux.jsonc",
  "set": { "name": "{{name}}", "env": "prod" }
}
```

- `file` is substituted at runtime (supports `{{...}}` variables)
- `set` passes/overrides variables into the sub-config's scope
- Parent scope values are inherited unless overridden by `set`
- Errors in the sub-config propagate normally (handlers, fallible, retries all work)

### exit

Terminate the run immediately with a given exit code and optional message:

```jsonc
{ "kind": "exit", "code": 0, "message": "Nothing to do — exiting." }
{ "kind": "exit", "code": 1, "message": "Unsupported platform: {{#os}}" }
```

- `code` defaults to 0 (success). Non-zero codes propagate as the process exit code.
- `message` is optional; substituted at runtime and printed before exiting.
- No subsequent steps run after an exit action fires.

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
  "retry-delay": 2.0,   // Seconds to wait before each retry
  "shell": "bash"       // Override shell (string shorthand or {cmd, args} object)
}
```

The `shell` field accepts a string shorthand (`"sh"`, `"bash"`, `"zsh"`, `"fish"`, `"cmd"`, `"powershell"`, `"pwsh"`) or an object `{ "cmd": "...", "args": [...] }`. It can be set at the top-level `meta` (global default) or per-step (overrides global).

### depends-on

Steps can declare prerequisites that are resolved transitively when using `--only`:

```jsonc
{
  "id": "deploy",
  "name": "Deploy",
  "depends-on": ["build", "test"],
  "action": { "kind": "shell", "commands": ["./deploy.sh"] }
}
```

- `rig config.json --only deploy` runs `build`, `test`, then `deploy`
- Dependencies are resolved transitively and deduplicated
- Cycles are rejected at parse time
- In normal sequential flow, `depends-on` has no effect (steps already run in order)

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
"{{#timestamp}}"          // %Y%m%dT%H%M%S at startup (e.g., 20260504T152259)
"{{#timestamp:%Y-%m-%d}}" // Custom strftime format
"{{#now}}"                // Current time, evaluated each use
"{{#now:%H:%M:%S}}"       // Custom strftime at each use
"{{#pwd}}"                // Current working directory at startup
"{{#os}}"                 // Operating system: linux, macos, windows
"{{#arch}}"               // CPU architecture: x86_64, aarch64, etc.
"{{#bundle}}"             // Absolute path to the bundle staging root (bundle runs only)
```

`{{#bundle}}` resolves to the temp/cwd/home/custom staging directory chosen
by the manifest's `bundle.extract-to` field. Outside a bundle run it stays
literal (like any other unresolved reference), which surfaces misuse in the
output.

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

Undefined non-`@` variables are rejected at parse time. To include a literal
`{{foo}}` in output (so it stays unsubstituted), wrap it in an extra pair of
braces on each side: `{{{{foo}}}}` renders to `{{foo}}`. The python-project
bundle uses this to reference its own templated directory names without
having them resolved at copy time.

## Examples

See [`examples/`](examples/) for sample configs.
