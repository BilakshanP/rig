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
    #[serde(default)]
    pub vars: HashMap<String, String>,
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
    let mut json = String::from_utf8_lossy(&buf).into_owned();

    // Extract meta.vars as literal defaults (before any substitution).
    let meta_vars = extract_meta_vars(&json);

    json = json.replace("\\{\\{", "\x00LBRACE\x00");

    // Built-in {{timestamp}} and {{timestamp:FORMAT}} variables.
    let now = chrono::Local::now();
    while let Some(start) = json.find("{{timestamp:") {
        let rest = &json[start + 12..];
        if let Some(end) = rest.find("}}") {
            let fmt = &rest[..end];
            let formatted = now.format(fmt).to_string();
            json = format!("{}{formatted}{}", &json[..start], &json[start + 12 + end + 2..]);
        } else {
            break;
        }
    }
    let default_ts = now.format("%Y%m%dT%H%M%S").to_string();
    json = json.replace("{{timestamp}}", &default_ts);

    // Merge: meta.vars provide defaults, CLI vars override.
    let mut final_vars = meta_vars;
    for (k, v) in cli_vars {
        final_vars.insert(k.clone(), v.clone());
    }

    for (key, val) in &final_vars {
        json = json.replace(&format!("{{{{{key}}}}}"), val);
    }

    if !placeholder
        && let Some(pos) = json.find("{{")
        && let Some(end) = json[pos + 2..].find("}}")
    {
        let var_name = &json[pos + 2..pos + 2 + end];
        return Err(ConfigError::UndefinedVar(var_name.to_string()));
    }

    json = json.replace("\x00LBRACE\x00", "{{");

    let config: Config = serde_json::from_str(&json)?;
    validate_unique_ids(&config)?;
    validate_refs(&config)?;
    validate_markup(&config)?;
    Ok(config)
}

/// Extract meta.vars from JSON without full parsing.
/// Returns empty map if not present or malformed.
fn extract_meta_vars(json: &str) -> HashMap<String, String> {
    let value: serde_json::Value = match serde_json::from_str(json) {
        Ok(v) => v,
        Err(_) => return HashMap::new(),
    };
    let vars = match value.pointer("/meta/vars") {
        Some(serde_json::Value::Object(m)) => m,
        _ => return HashMap::new(),
    };
    vars.iter()
        .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
        .collect()
}

/// Scan config text for all {{var}} references (for --vars listing).
/// Returns a sorted list of unique variable names, excluding {{timestamp}} and {{timestamp:...}}.
pub fn scan_vars(path: &str) -> Result<Vec<String>, ConfigError> {
    let content = std::fs::read_to_string(path)?;
    let mut buf = Vec::new();
    io::Read::read_to_end(&mut json_comments::StripComments::new(content.as_bytes()), &mut buf)?;
    let json = String::from_utf8_lossy(&buf).replace("\\{\\{", "");

    let mut vars = std::collections::BTreeSet::new();
    let mut rest = json.as_str();
    while let Some(start) = rest.find("{{") {
        let after = &rest[start + 2..];
        if let Some(end) = after.find("}}") {
            let name = &after[..end];
            // Skip timestamp built-in
            if name != "timestamp" && !name.starts_with("timestamp:") {
                vars.insert(name.to_string());
            }
            rest = &after[end + 2..];
        } else {
            break;
        }
    }
    Ok(vars.into_iter().collect())
}

/// Extract just meta.vars from a config file (for --vars listing).
/// Values are returned as literal strings (no substitution performed).
pub fn read_meta_vars(path: &str) -> Result<HashMap<String, String>, ConfigError> {
    let content = std::fs::read_to_string(path)?;
    let mut buf = Vec::new();
    io::Read::read_to_end(&mut json_comments::StripComments::new(content.as_bytes()), &mut buf)?;
    let json = String::from_utf8_lossy(&buf).into_owned();
    Ok(extract_meta_vars(&json))
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
        let json = r#"{
            "name": "{{project}}", "version": "1.0.0",
            "steps": [{ "name": "run", "action": { "kind": "shell", "commands": ["echo {{greeting}}"] } }]
        }"#;
        let path = std::env::temp_dir().join("rig_var_test.json");
        std::fs::write(&path, json).unwrap();
        let vars = HashMap::from([("project".into(), "my-app".into()), ("greeting".into(), "hello".into())]);
        let cfg = parse_config(path.to_str().unwrap(), &vars, false).unwrap();
        assert_eq!(cfg.name, "my-app");
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
        assert_eq!(cfg.name, "default-name");
        assert_eq!(cfg.meta.vars.get("env").unwrap(), "dev");
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
        assert_eq!(cfg.name, "overridden");
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
        assert_eq!(vars, vec!["env".to_string(), "greeting".to_string(), "project".to_string()]);
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn scan_vars_excludes_timestamp() {
        let json = r#"{
            "name": "{{project}}-{{timestamp}}-{{timestamp:%Y}}", "version": "1.0.0",
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
            Err(ConfigError::UndefinedVar(v)) if v == "missing"
        ));
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
        let json = r#"{ "name": "run-{{timestamp:%Y-%m-%d}}", "version": "1.0.0", "steps": [] }"#;
        let path = std::env::temp_dir().join("rig_tsfmt_test.json");
        std::fs::write(&path, json).unwrap();
        let cfg = parse_config(path.to_str().unwrap(), &HashMap::new(), false).unwrap();
        assert!(cfg.name.starts_with("run-20"));
        assert_eq!(cfg.name.len(), "run-2026-05-04".len());
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
