use serde::{Deserialize, Deserializer};
use std::collections::HashMap;
use std::fmt;
use std::io;

// -- Top-level config --

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct Config {
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub meta: Meta,
    pub steps: Vec<Step>,
}

// -- Top-level meta --

#[derive(Debug, Clone, Default, Deserialize)]
#[allow(dead_code)]
pub struct Meta {
    #[serde(default)]
    pub log: Option<String>,
    #[serde(default)]
    pub silent: Vec<Silent>,
    #[serde(default)]
    pub sudo: bool,
    #[serde(default)]
    pub retries: Option<u32>,
    #[serde(default, rename = "retry-delay")]
    pub retry_delay: Option<f64>,
    /// Raw vars tree. May contain nested objects; flattened into dot-path keys at runtime.
    #[serde(default)]
    pub vars: serde_json::Value,
}

// -- Step --

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct Step {
    #[serde(default)]
    pub id: Option<String>,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    pub action: Action,
    #[serde(default, rename = "on-success", deserialize_with = "de_opt_single_or_vec")]
    pub on_success: Option<Vec<StepRef>>,
    #[serde(default, rename = "on-failure", deserialize_with = "de_opt_single_or_vec")]
    pub on_failure: Option<Vec<StepRef>>,
    #[serde(default, rename = "on-return", deserialize_with = "de_opt_return_map")]
    pub on_return: Option<HashMap<String, Vec<StepRef>>>,
    #[serde(default)]
    pub then: Vec<StepRef>,
    #[serde(default)]
    pub meta: StepMeta,
}

// -- Step references (id string or inline step) --

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum StepRef {
    Id(String),
    Inline(Box<Step>),
}

// -- Step meta --

#[derive(Debug, Clone, Default, Deserialize)]
#[allow(dead_code)]
pub struct StepMeta {
    #[serde(default)]
    pub optional: bool,
    #[serde(default)]
    pub fallible: bool,
    #[serde(default)]
    pub sudo: bool,
    #[serde(default)]
    pub silent: Vec<Silent>,
    #[serde(default)]
    pub retries: Option<u32>,
    #[serde(default, rename = "retry-delay")]
    pub retry_delay: Option<f64>,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Silent {
    Stdout,
    Stderr,
}

// -- Action (tagged by "kind") --

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Action {
    Shell {
        #[serde(deserialize_with = "de_single_or_vec")]
        commands: Vec<String>,
        #[serde(default)]
        dir: Option<String>,
        #[serde(default)]
        env: Option<HashMap<String, String>>,
    },
    Git {
        repo: String,
        dest: String,
        #[serde(default, rename = "on-conflict")]
        on_conflict: GitOnConflict,
    },
    Fs {
        #[serde(flatten)]
        op: FsOp,
        #[serde(default, rename = "if-exists")]
        if_exists: Option<Condition>,
        #[serde(default, rename = "if-not-exists")]
        if_not_exists: Option<Condition>,
    },
    Io {
        level: IoLevel,
        message: String,
        #[serde(default)]
        markup: bool,
    },
    Var {
        /// Variable name (must include @ prefix).
        name: String,
        #[serde(flatten)]
        source: VarSource,
    },
}

// -- Var --

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum VarSource {
    /// Run the referenced step and capture its stdout.
    From { from: StepRef },
    /// Run the referenced step, feeding the variable's current value as stdin.
    To { to: StepRef },
    /// Run a shell command and capture its stdout directly.
    Command { command: String },
    /// Read a file's contents into the variable.
    File { file: String },
}

// -- IO --

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum IoLevel {
    Log,
    Info,
    Warn,
    Error,
}

// -- Git --

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum GitOnConflict {
    #[default]
    Skip,
    Pull,
    Fail,
}

// -- FS --

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FsOp {
    Create {
        #[serde(deserialize_with = "de_single_or_vec")]
        path: Vec<String>,
        #[serde(default)]
        recurse: bool,
        #[serde(default)]
        content: Option<String>,
    },
    Symlink {
        from: String,
        to: String,
    },
    Copy {
        from: String,
        to: String,
    },
    Move {
        from: String,
        to: String,
    },
    Delete {
        #[serde(deserialize_with = "de_single_or_vec")]
        path: Vec<String>,
        #[serde(default)]
        recurse: bool,
    },
}

// -- Condition (if-exists / if-not-exists) --

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum Condition {
    Action(ConditionAction),
    Execute { execute: StepRef },
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ConditionAction {
    Skip,
    Overwrite,
    Append,
    Panic,
}

// -- Serde helpers --

/// Deserialize a field that can be either a single value or an array into Vec<T>.
fn de_single_or_vec<'de, T, D>(d: D) -> Result<Vec<T>, D::Error>
where
    T: Deserialize<'de>,
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum OneOrMany<T> {
        One(T),
        Many(Vec<T>),
    }
    match OneOrMany::<T>::deserialize(d)? {
        OneOrMany::One(v) => Ok(vec![v]),
        OneOrMany::Many(v) => Ok(v),
    }
}

fn de_opt_single_or_vec<'de, T, D>(d: D) -> Result<Option<Vec<T>>, D::Error>
where
    T: Deserialize<'de>,
    D: Deserializer<'de>,
{
    Ok(Some(de_single_or_vec(d)?))
}

/// Deserialize a HashMap<String, StepRef | Vec<StepRef>> into HashMap<String, Vec<StepRef>>.
fn de_opt_return_map<'de, D>(d: D) -> Result<Option<HashMap<String, Vec<StepRef>>>, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum OneOrMany {
        One(StepRef),
        Many(Vec<StepRef>),
    }
    let raw: HashMap<String, OneOrMany> = HashMap::deserialize(d)?;
    Ok(Some(raw.into_iter().map(|(k, v)| {
        let v = match v {
            OneOrMany::One(sr) => vec![sr],
            OneOrMany::Many(v) => v,
        };
        (k, v)
    }).collect()))
}

// -- Errors --

#[derive(Debug)]
pub enum ConfigError {
    Io(io::Error),
    Parse(serde_json::Error),
    DuplicateId(String),
    UnknownRef(String),
    UndefinedVar(String),
    InvalidMarkup(String, String),
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "failed to read config: {e}"),
            Self::Parse(e) => write!(f, "failed to parse config: {e}"),
            Self::DuplicateId(id) => write!(f, "duplicate step id: {id}"),
            Self::UnknownRef(id) => write!(f, "unknown step reference: {id}"),
            Self::UndefinedVar(name) => write!(f, "undefined variable: {name}"),
            Self::InvalidMarkup(step, msg) => write!(f, "invalid aml markup in step '{step}': {msg}"),
        }
    }
}

impl From<io::Error> for ConfigError {
    fn from(e: io::Error) -> Self { Self::Io(e) }
}

impl From<serde_json::Error> for ConfigError {
    fn from(e: serde_json::Error) -> Self { Self::Parse(e) }
}

// -- Parser --

pub fn parse_config(path: &str, cli_vars: &HashMap<String, String>, placeholder: bool) -> Result<Config, ConfigError> {
    let content = std::fs::read_to_string(path)?;
    let mut buf = Vec::new();
    io::Read::read_to_end(&mut json_comments::StripComments::new(content.as_bytes()), &mut buf)?;
    let json = String::from_utf8_lossy(&buf).into_owned();

    let config: Config = serde_json::from_str(&json)?;
    validate_unique_ids(&config)?;
    validate_refs(&config)?;
    validate_markup(&config)?;
    validate_vars(&config, cli_vars, placeholder)?;
    Ok(config)
}

/// Validate variable usage: check that all referenced names are either known constants,
/// built-ins, or declared @-vars (which will be provided by var actions or --set).
fn validate_vars(config: &Config, cli_vars: &HashMap<String, String>, placeholder: bool) -> Result<(), ConfigError> {
    if placeholder { return Ok(()); }

    let meta_vars = crate::vars::flatten_vars(&config.meta.vars);

    // Collect all referenced vars from all string fields in the config.
    let mut referenced: Vec<crate::vars::VarRef> = Vec::new();
    collect_refs_in_config(config, &mut referenced);

    for vr in &referenced {
        use crate::vars::VarKind;
        match vr.kind() {
            VarKind::BuiltinStartup | VarKind::BuiltinRuntime => {
                // Built-ins are always fine (we handle known names at resolve time).
                // We could reject unknown #names here, but leave that as runtime error.
            }
            VarKind::ConstUpper => {
                // Must be defined in meta.vars.
                if !meta_vars.contains_key(&vr.key()) {
                    return Err(ConfigError::UndefinedVar(vr.display()));
                }
            }
            VarKind::ConstLower => {
                // Defined in meta.vars or via --set.
                if !meta_vars.contains_key(&vr.key()) && !cli_vars.contains_key(&vr.key()) {
                    return Err(ConfigError::UndefinedVar(vr.display()));
                }
            }
            VarKind::MutableUpper | VarKind::MutableLower => {
                // Runtime-mutable. No parse-time check needed -- may be set by var action.
                // Initial value from meta.vars or (for lowercase) --set is optional.
            }
        }
    }

    // Also: any `var` action targeting an immutable var is a parse-time error.
    for step in &config.steps { check_var_action_writes(step)?; }

    Ok(())
}

fn check_var_action_writes(step: &Step) -> Result<(), ConfigError> {
    if let Action::Var { name, .. } = &step.action {
        match crate::vars::VarRef::parse(name) {
            Some(vr) if vr.is_runtime_writable() => {}
            Some(vr) => {
                return Err(ConfigError::UndefinedVar(format!(
                    "var action target '{}' is not runtime-writable (use @-prefix)",
                    vr.display()
                )));
            }
            None => return Err(ConfigError::UndefinedVar(format!("invalid var action target: {name}"))),
        }
    }
    for child in &step.then {
        if let StepRef::Inline(s) = child { check_var_action_writes(s)?; }
    }
    Ok(())
}

/// Walk the entire config and collect all {{var}} references from string fields.
fn collect_refs_in_config(config: &Config, refs: &mut Vec<crate::vars::VarRef>) {
    refs.extend(crate::vars::scan_refs(&config.name));
    if let Some(d) = &config.description { refs.extend(crate::vars::scan_refs(d)); }
    if let Some(log) = &config.meta.log {
        refs.extend(crate::vars::scan_refs(log));
    }
    for step in &config.steps { collect_refs_in_step(step, refs); }
}

fn collect_refs_in_step(step: &Step, refs: &mut Vec<crate::vars::VarRef>) {
    refs.extend(crate::vars::scan_refs(&step.name));
    if let Some(d) = &step.description { refs.extend(crate::vars::scan_refs(d)); }
    collect_refs_in_action(&step.action, refs);
    for child in &step.then {
        if let StepRef::Inline(s) = child { collect_refs_in_step(s, refs); }
    }
}

fn collect_refs_in_action(action: &Action, refs: &mut Vec<crate::vars::VarRef>) {
    match action {
        Action::Shell { commands, dir, env } => {
            for c in commands { refs.extend(crate::vars::scan_refs(c)); }
            if let Some(d) = dir { refs.extend(crate::vars::scan_refs(d)); }
            if let Some(e) = env {
                for v in e.values() { refs.extend(crate::vars::scan_refs(v)); }
            }
        }
        Action::Git { repo, dest, .. } => {
            refs.extend(crate::vars::scan_refs(repo));
            refs.extend(crate::vars::scan_refs(dest));
        }
        Action::Fs { op, .. } => match op {
            FsOp::Create { path, content, .. } => {
                for p in path { refs.extend(crate::vars::scan_refs(p)); }
                if let Some(c) = content { refs.extend(crate::vars::scan_refs(c)); }
            }
            FsOp::Delete { path, .. } => {
                for p in path { refs.extend(crate::vars::scan_refs(p)); }
            }
            FsOp::Symlink { from, to } | FsOp::Copy { from, to } | FsOp::Move { from, to } => {
                refs.extend(crate::vars::scan_refs(from));
                refs.extend(crate::vars::scan_refs(to));
            }
        },
        Action::Io { message, .. } => refs.extend(crate::vars::scan_refs(message)),
        Action::Var { source, .. } => match source {
            VarSource::Command { command } => refs.extend(crate::vars::scan_refs(command)),
            VarSource::File { file } => refs.extend(crate::vars::scan_refs(file)),
            _ => {}
        },
    }
}

// -- Validation --

fn validate_unique_ids(config: &Config) -> Result<(), ConfigError> {
    let mut seen = std::collections::HashSet::new();
    for step in &config.steps { collect_ids(step, &mut seen)?; }
    Ok(())
}

fn collect_ids(step: &Step, seen: &mut std::collections::HashSet<String>) -> Result<(), ConfigError> {
    if let Some(id) = &step.id
        && !seen.insert(id.clone())
    {
        return Err(ConfigError::DuplicateId(id.clone()));
    }
    for child in &step.then {
        if let StepRef::Inline(s) = child { collect_ids(s, seen)?; }
    }
    visit_step_refs(step, &mut |sr| {
        if let StepRef::Inline(s) = sr { collect_ids(s, seen)?; }
        Ok(())
    })
}

/// Build the initial runtime scope from meta.vars + CLI --set overrides.
pub fn build_scope(config: &Config, cli_vars: &HashMap<String, String>) -> crate::vars::Scope {
    let pwd = std::env::current_dir().map(|p| p.display().to_string()).unwrap_or_default();
    let mut scope = crate::vars::Scope::new(chrono::Local::now(), pwd);

    // Flatten meta.vars and populate the scope (only string/number/bool leaves).
    let flat = crate::vars::flatten_vars(&config.meta.vars);
    for (k, v) in flat { scope.set(&k, v); }

    // Overlay CLI vars (only allowed on `name` or `@name` categories, but we don't
    // enforce at this layer -- validate_vars would catch misuse).
    for (k, v) in cli_vars { scope.set(k, v.clone()); }

    scope
}

/// Build a map of id -> Step for reference resolution.
pub fn build_step_index(config: &Config) -> HashMap<String, Step> {
    let mut map = HashMap::new();
    for step in &config.steps { index_step(step, &mut map); }
    map
}

fn index_step(step: &Step, map: &mut HashMap<String, Step>) {
    if let Some(id) = &step.id { map.insert(id.clone(), step.clone()); }
    for child in &step.then {
        if let StepRef::Inline(s) = child { index_step(s, map); }
    }
}

/// Scan a config file for all referenced variable keys (for --vars display).
pub fn scan_vars(path: &str) -> Result<Vec<String>, ConfigError> {
    let content = std::fs::read_to_string(path)?;
    let mut buf = Vec::new();
    io::Read::read_to_end(&mut json_comments::StripComments::new(content.as_bytes()), &mut buf)?;
    let json = String::from_utf8_lossy(&buf).into_owned();
    let mut set = std::collections::BTreeSet::new();
    for vr in crate::vars::scan_refs(&json) {
        // Skip built-ins from the listing.
        if matches!(vr.kind(), crate::vars::VarKind::BuiltinStartup | crate::vars::VarKind::BuiltinRuntime) { continue; }
        set.insert(vr.display());
    }
    Ok(set.into_iter().collect())
}

/// Extract meta.vars from a config file (for --vars listing defaults).
pub fn read_meta_vars(path: &str) -> Result<HashMap<String, String>, ConfigError> {
    let content = std::fs::read_to_string(path)?;
    let mut buf = Vec::new();
    io::Read::read_to_end(&mut json_comments::StripComments::new(content.as_bytes()), &mut buf)?;
    let json = String::from_utf8_lossy(&buf).into_owned();
    let value: serde_json::Value = serde_json::from_str(&json)?;
    let vars = match value.pointer("/meta/vars") {
        Some(v) => v.clone(),
        None => return Ok(HashMap::new()),
    };
    Ok(crate::vars::flatten_vars(&vars))
}

fn validate_refs(config: &Config) -> Result<(), ConfigError> {
    let mut ids = std::collections::HashSet::new();
    for step in &config.steps { collect_all_ids(step, &mut ids); }
    for step in &config.steps { check_refs(step, &ids)?; }
    Ok(())
}

fn collect_all_ids(step: &Step, ids: &mut std::collections::HashSet<String>) {
    if let Some(id) = &step.id { ids.insert(id.clone()); }
    for child in &step.then {
        if let StepRef::Inline(s) = child { collect_all_ids(s, ids); }
    }
    let _ = visit_step_refs(step, &mut |sr| {
        if let StepRef::Inline(s) = sr { collect_all_ids(s, ids); }
        Ok(())
    });
}

fn check_refs(step: &Step, ids: &std::collections::HashSet<String>) -> Result<(), ConfigError> {
    for child in &step.then { check_step_ref(child, ids)?; }
    visit_step_refs(step, &mut |sr| check_step_ref(sr, ids))
}

fn check_step_ref(sr: &StepRef, ids: &std::collections::HashSet<String>) -> Result<(), ConfigError> {
    match sr {
        StepRef::Id(id) => { if !ids.contains(id) { Err(ConfigError::UnknownRef(id.clone())) } else { Ok(()) } }
        StepRef::Inline(s) => check_refs(s, ids),
    }
}

/// Visit all StepRef values in a step's handlers and conditions.
fn visit_step_refs(step: &Step, f: &mut impl FnMut(&StepRef) -> Result<(), ConfigError>) -> Result<(), ConfigError> {
    if let Some(refs) = &step.on_success { for sr in refs { f(sr)?; } }
    if let Some(refs) = &step.on_failure { for sr in refs { f(sr)?; } }
    if let Some(map) = &step.on_return {
        for refs in map.values() { for sr in refs { f(sr)?; } }
    }
    if let Action::Fs { if_exists: Some(Condition::Execute { execute }), .. } = &step.action { f(execute)?; }
    if let Action::Fs { if_not_exists: Some(Condition::Execute { execute }), .. } = &step.action { f(execute)?; }
    Ok(())
}

fn validate_markup(config: &Config) -> Result<(), ConfigError> {
    for step in &config.steps { check_markup(step)?; }
    Ok(())
}

fn check_markup(step: &Step) -> Result<(), ConfigError> {
    if let Action::Io { markup: true, message, .. } = &step.action {
        use aml::prelude::Document;
        if Document::try_new(message).is_err() {
            return Err(ConfigError::InvalidMarkup(step.name.clone(), message.clone()));
        }
    }
    for child in &step.then {
        if let StepRef::Inline(s) = child { check_markup(s)?; }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_shell_action() {
        let json = r#"{
            "name": "test", "version": "1.0.0",
            "steps": [{
                "name": "run",
                "action": { "kind": "shell", "commands": ["echo hi"], "dir": "~", "env": {"FOO": "bar"} }
            }]
        }"#;
        let cfg: Config = serde_json::from_str(json).unwrap();
        match &cfg.steps[0].action {
            Action::Shell { commands, dir, env } => {
                assert_eq!(commands, &["echo hi"]);
                assert_eq!(dir.as_deref(), Some("~"));
                assert_eq!(env.as_ref().unwrap()["FOO"], "bar");
            }
            _ => panic!("expected Shell"),
        }
    }

    #[test]
    fn parse_shell_single_command_string() {
        let json = r#"{
            "name": "test", "version": "1.0.0",
            "steps": [{
                "name": "run",
                "action": { "kind": "shell", "commands": "echo hi" }
            }]
        }"#;
        let cfg: Config = serde_json::from_str(json).unwrap();
        match &cfg.steps[0].action {
            Action::Shell { commands, .. } => assert_eq!(commands, &["echo hi"]),
            _ => panic!("expected Shell"),
        }
    }

    #[test]
    fn parse_git_action() {
        let json = r#"{
            "name": "test", "version": "1.0.0",
            "steps": [{
                "name": "clone",
                "action": { "kind": "git", "repo": "https://github.com/user/repo.git", "dest": "~/.dotfiles", "on-conflict": "pull" }
            }]
        }"#;
        let cfg: Config = serde_json::from_str(json).unwrap();
        match &cfg.steps[0].action {
            Action::Git { repo, dest, on_conflict } => {
                assert_eq!(repo, "https://github.com/user/repo.git");
                assert_eq!(dest, "~/.dotfiles");
                assert_eq!(*on_conflict, GitOnConflict::Pull);
            }
            _ => panic!("expected Git"),
        }
    }

    #[test]
    fn parse_fs_create_single_path() {
        let json = r#"{
            "name": "test", "version": "1.0.0",
            "steps": [{
                "name": "create",
                "action": { "kind": "fs", "create": { "path": "~/projects/", "recurse": true }, "if-exists": "skip" }
            }]
        }"#;
        let cfg: Config = serde_json::from_str(json).unwrap();
        match &cfg.steps[0].action {
            Action::Fs { op: FsOp::Create { path, recurse, .. }, if_exists, .. } => {
                assert_eq!(path, &["~/projects/"]);
                assert!(recurse);
                assert!(matches!(if_exists, Some(Condition::Action(ConditionAction::Skip))));
            }
            _ => panic!("expected Fs Create"),
        }
    }

    #[test]
    fn parse_fs_create_multiple_paths() {
        let json = r#"{
            "name": "test", "version": "1.0.0",
            "steps": [{
                "name": "create",
                "action": { "kind": "fs", "create": { "path": ["~/a/", "~/b/"], "recurse": true } }
            }]
        }"#;
        let cfg: Config = serde_json::from_str(json).unwrap();
        match &cfg.steps[0].action {
            Action::Fs { op: FsOp::Create { path, .. }, .. } => {
                assert_eq!(path.len(), 2);
            }
            _ => panic!("expected Fs Create"),
        }
    }

    #[test]
    fn parse_fs_create_with_content() {
        let json = r#"{
            "name": "test", "version": "1.0.0",
            "steps": [{
                "name": "create",
                "action": { "kind": "fs", "create": { "path": "~/file.txt", "content": "hello world" } }
            }]
        }"#;
        let cfg: Config = serde_json::from_str(json).unwrap();
        match &cfg.steps[0].action {
            Action::Fs { op: FsOp::Create { content, .. }, .. } => {
                assert_eq!(content.as_deref(), Some("hello world"));
            }
            _ => panic!("expected Fs Create"),
        }
    }

    #[test]
    fn parse_fs_symlink() {
        let json = r#"{
            "name": "test", "version": "1.0.0",
            "steps": [{
                "name": "link",
                "action": { "kind": "fs", "symlink": { "from": "~/a", "to": "~/b" }, "if-exists": "overwrite" }
            }]
        }"#;
        let cfg: Config = serde_json::from_str(json).unwrap();
        match &cfg.steps[0].action {
            Action::Fs { op: FsOp::Symlink { from, to }, .. } => {
                assert_eq!(from, "~/a");
                assert_eq!(to, "~/b");
            }
            _ => panic!("expected Fs Symlink"),
        }
    }

    #[test]
    fn parse_on_success_single() {
        let json = r#"{
            "name": "test", "version": "1.0.0",
            "steps": [{
                "name": "run",
                "action": { "kind": "shell", "commands": ["echo"] },
                "on-success": "next-step"
            }]
        }"#;
        let cfg: Config = serde_json::from_str(json).unwrap();
        let refs = cfg.steps[0].on_success.as_ref().unwrap();
        assert_eq!(refs.len(), 1);
        assert!(matches!(&refs[0], StepRef::Id(id) if id == "next-step"));
    }

    #[test]
    fn parse_on_success_array() {
        let json = r#"{
            "name": "test", "version": "1.0.0",
            "steps": [{
                "name": "run",
                "action": { "kind": "shell", "commands": ["echo"] },
                "on-success": ["step-a", "step-b"]
            }]
        }"#;
        let cfg: Config = serde_json::from_str(json).unwrap();
        let refs = cfg.steps[0].on_success.as_ref().unwrap();
        assert_eq!(refs.len(), 2);
    }

    #[test]
    fn parse_on_return() {
        let json = r#"{
            "name": "test", "version": "1.0.0",
            "steps": [{
                "name": "run",
                "action": { "kind": "shell", "commands": ["echo"] },
                "on-return": { "0": "success-step", "_": ["a", "b"] }
            }]
        }"#;
        let cfg: Config = serde_json::from_str(json).unwrap();
        let map = cfg.steps[0].on_return.as_ref().unwrap();
        assert_eq!(map["0"].len(), 1);
        assert_eq!(map["_"].len(), 2);
    }

    #[test]
    fn parse_meta() {
        let json = r#"{
            "name": "test", "version": "1.0.0",
            "steps": [{
                "name": "run",
                "action": { "kind": "shell", "commands": ["echo"] },
                "meta": { "optional": true, "fallible": true, "silent": ["stdout"], "retries": 3, "retry-delay": 2.0 }
            }]
        }"#;
        let cfg: Config = serde_json::from_str(json).unwrap();
        let meta = &cfg.steps[0].meta;
        assert!(meta.optional);
        assert!(meta.fallible);
        assert_eq!(meta.retries, Some(3));
    }

    #[test]
    fn parse_top_level_meta() {
        let json = r#"{
            "name": "test", "version": "1.0.0",
            "meta": { "log": "/tmp/a.log", "silent": ["stdout"], "sudo": true, "retries": 2, "retry-delay": 1.5 },
            "steps": []
        }"#;
        let cfg: Config = serde_json::from_str(json).unwrap();
        assert_eq!(cfg.meta.log.as_deref(), Some("/tmp/a.log"));
        assert!(cfg.meta.sudo);
        assert_eq!(cfg.meta.retries, Some(2));
    }

    #[test]
    fn parse_then_mixed() {
        let json = r#"{
            "name": "test", "version": "1.0.0",
            "steps": [{
                "name": "parent",
                "action": { "kind": "shell", "commands": ["echo"] },
                "then": [
                    "ref-id",
                    { "name": "inline", "action": { "kind": "shell", "commands": ["echo inline"] } }
                ]
            }]
        }"#;
        let cfg: Config = serde_json::from_str(json).unwrap();
        assert_eq!(cfg.steps[0].then.len(), 2);
        assert!(matches!(&cfg.steps[0].then[0], StepRef::Id(id) if id == "ref-id"));
        assert!(matches!(&cfg.steps[0].then[1], StepRef::Inline(_)));
    }

    #[test]
    fn parse_io_action() {
        let json = r#"{
            "name": "test", "version": "1.0.0",
            "steps": [{
                "name": "log",
                "action": { "kind": "io", "level": "info", "message": "hello" }
            }]
        }"#;
        let cfg: Config = serde_json::from_str(json).unwrap();
        assert!(matches!(&cfg.steps[0].action, Action::Io { level: IoLevel::Info, .. }));
    }

    #[test]
    fn duplicate_ids_rejected() {
        let json = r#"{
            "name": "test", "version": "1.0.0",
            "steps": [
                { "id": "dup", "name": "a", "action": { "kind": "shell", "commands": ["echo"] } },
                { "id": "dup", "name": "b", "action": { "kind": "shell", "commands": ["echo"] } }
            ]
        }"#;
        let path = std::env::temp_dir().join("rig_dup_test.json");
        std::fs::write(&path, json).unwrap();
        assert!(matches!(parse_config(path.to_str().unwrap(), &HashMap::new(), false), Err(ConfigError::DuplicateId(_))));
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn unknown_ref_rejected() {
        let json = r#"{
            "name": "test", "version": "1.0.0",
            "steps": [{
                "name": "parent",
                "action": { "kind": "shell", "commands": ["echo"] },
                "then": ["nonexistent"]
            }]
        }"#;
        let path = std::env::temp_dir().join("rig_unknownref_test.json");
        std::fs::write(&path, json).unwrap();
        assert!(matches!(parse_config(path.to_str().unwrap(), &HashMap::new(), false), Err(ConfigError::UnknownRef(_))));
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn valid_refs_accepted() {
        let json = r#"{
            "name": "test", "version": "1.0.0",
            "steps": [
                { "id": "a", "name": "a", "action": { "kind": "shell", "commands": ["echo"] } },
                { "name": "b", "action": { "kind": "shell", "commands": ["echo"] }, "then": ["a"] }
            ]
        }"#;
        let path = std::env::temp_dir().join("rig_validref_test.json");
        std::fs::write(&path, json).unwrap();
        assert!(parse_config(path.to_str().unwrap(), &HashMap::new(), false).is_ok());
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn var_substitution() {
        // In the new model, parse_config leaves {{vars}} unresolved.
        // Substitution happens at runtime via Scope.
        let json = r#"{
            "name": "{{project}}", "version": "1.0.0",
            "meta": { "vars": { "project": "my-app", "greeting": "hello" } },
            "steps": [{ "name": "run", "action": { "kind": "shell", "commands": ["echo {{greeting}}"] } }]
        }"#;
        let path = std::env::temp_dir().join("rig_var_test.json");
        std::fs::write(&path, json).unwrap();
        let cfg = parse_config(path.to_str().unwrap(), &HashMap::new(), false).unwrap();
        // Name is still the raw template...
        assert_eq!(cfg.name, "{{project}}");
        // ...but a scope built from it resolves correctly.
        let scope = build_scope(&cfg, &HashMap::new());
        assert_eq!(scope.substitute(&cfg.name), "my-app");
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn meta_vars_as_default() {
        let json = r#"{
            "name": "{{project}}", "version": "1.0.0",
            "meta": { "vars": { "project": "default-name", "env": "dev" } },
            "steps": []
        }"#;
        let path = std::env::temp_dir().join("rig_meta_vars_test.json");
        std::fs::write(&path, json).unwrap();
        let cfg = parse_config(path.to_str().unwrap(), &HashMap::new(), false).unwrap();
        let scope = build_scope(&cfg, &HashMap::new());
        assert_eq!(scope.substitute(&cfg.name), "default-name");
        assert_eq!(scope.get("env"), Some("dev"));
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn cli_vars_override_meta_vars() {
        let json = r#"{
            "name": "{{project}}", "version": "1.0.0",
            "meta": { "vars": { "project": "default-name" } },
            "steps": []
        }"#;
        let path = std::env::temp_dir().join("rig_override_test.json");
        std::fs::write(&path, json).unwrap();
        let cli_vars = HashMap::from([("project".into(), "overridden".into())]);
        let cfg = parse_config(path.to_str().unwrap(), &cli_vars, false).unwrap();
        let scope = build_scope(&cfg, &cli_vars);
        assert_eq!(scope.substitute(&cfg.name), "overridden");
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn scan_vars_finds_all_references() {
        let json = r#"{
            "name": "{{project}}", "version": "1.0.0",
            "steps": [{ "name": "s", "action": { "kind": "shell", "commands": ["echo {{greeting}} {{env}}"] } }]
        }"#;
        let path = std::env::temp_dir().join("rig_scan_test.json");
        std::fs::write(&path, json).unwrap();
        let vars = scan_vars(path.to_str().unwrap()).unwrap();
        // scan_vars returns VarRef::display() strings including any prefix
        assert!(vars.contains(&"project".to_string()));
        assert!(vars.contains(&"greeting".to_string()));
        assert!(vars.contains(&"env".to_string()));
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn scan_vars_excludes_timestamp() {
        let json = r#"{
            "name": "{{project}}-{{#timestamp}}-{{#timestamp:%Y}}", "version": "1.0.0",
            "meta": { "vars": { "project": "p" } },
            "steps": []
        }"#;
        let path = std::env::temp_dir().join("rig_scan_ts_test.json");
        std::fs::write(&path, json).unwrap();
        let vars = scan_vars(path.to_str().unwrap()).unwrap();
        assert_eq!(vars, vec!["project".to_string()]);
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn undefined_var_rejected() {
        let json = r#"{ "name": "{{missing}}", "version": "1.0.0", "steps": [] }"#;
        let path = std::env::temp_dir().join("rig_undef_test.json");
        std::fs::write(&path, json).unwrap();
        assert!(matches!(
            parse_config(path.to_str().unwrap(), &HashMap::new(), false),
            Err(ConfigError::UndefinedVar(_))
        ));
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn at_var_not_rejected_without_definition() {
        // @vars are runtime-mutable, so they don't need to be defined at parse time.
        let json = r#"{
            "name": "test", "version": "1.0.0",
            "steps": [{ "name": "use", "action": { "kind": "shell", "commands": ["echo {{@result}}"] } }]
        }"#;
        let path = std::env::temp_dir().join("rig_atvar_test.json");
        std::fs::write(&path, json).unwrap();
        assert!(parse_config(path.to_str().unwrap(), &HashMap::new(), false).is_ok());
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn write_to_immutable_rejected() {
        let json = r#"{
            "name": "test", "version": "1.0.0",
            "steps": [{ "name": "bad", "action": { "kind": "var", "name": "result", "command": "echo x" } }]
        }"#;
        let path = std::env::temp_dir().join("rig_immut_write_test.json");
        std::fs::write(&path, json).unwrap();
        assert!(parse_config(path.to_str().unwrap(), &HashMap::new(), false).is_err());
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn write_to_at_var_accepted() {
        let json = r#"{
            "name": "test", "version": "1.0.0",
            "steps": [{ "name": "ok", "action": { "kind": "var", "name": "@result", "command": "echo x" } }]
        }"#;
        let path = std::env::temp_dir().join("rig_atvar_write_test.json");
        std::fs::write(&path, json).unwrap();
        assert!(parse_config(path.to_str().unwrap(), &HashMap::new(), false).is_ok());
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn placeholder_mode_keeps_undefined_vars() {
        let json = r#"{ "name": "{{missing}}", "version": "1.0.0", "steps": [] }"#;
        let path = std::env::temp_dir().join("rig_placeholder_test.json");
        std::fs::write(&path, json).unwrap();
        let cfg = parse_config(path.to_str().unwrap(), &HashMap::new(), true).unwrap();
        assert_eq!(cfg.name, "{{missing}}");
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn timestamp_custom_format() {
        let json = r#"{ "name": "run-{{#timestamp:%Y-%m-%d}}", "version": "1.0.0", "steps": [] }"#;
        let path = std::env::temp_dir().join("rig_tsfmt_test.json");
        std::fs::write(&path, json).unwrap();
        let cfg = parse_config(path.to_str().unwrap(), &HashMap::new(), false).unwrap();
        // Template preserved at parse time.
        assert_eq!(cfg.name, "run-{{#timestamp:%Y-%m-%d}}");
        // Scope resolves it.
        let scope = build_scope(&cfg, &HashMap::new());
        let resolved = scope.substitute(&cfg.name);
        assert!(resolved.starts_with("run-20"));
        assert_eq!(resolved.len(), "run-2026-05-04".len());
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn invalid_markup_rejected() {
        let json = r#"{
            "name": "test", "version": "1.0.0",
            "steps": [{ "name": "bad", "action": { "kind": "io", "level": "info", "message": "<invalid_tag>oops", "markup": true } }]
        }"#;
        let path = std::env::temp_dir().join("rig_markup_bad_test.json");
        std::fs::write(&path, json).unwrap();
        assert!(matches!(
            parse_config(path.to_str().unwrap(), &HashMap::new(), false),
            Err(ConfigError::InvalidMarkup(..))
        ));
        std::fs::remove_file(path).ok();
    }
}
