use serde::Deserialize;
use std::collections::HashMap;
use std::fmt;
use std::io;

// ── Top-level config ──

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct Config {
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub meta: ConfigMeta,
    pub steps: Vec<Step>,
}

// ── Top-level meta ──

#[derive(Debug, Clone, Default, Deserialize)]
#[allow(dead_code)]
pub struct ConfigMeta {
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
}

// ── Step ──

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
pub struct Step {
    #[serde(default)]
    pub id: Option<String>,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    pub action: Action,
    #[serde(default, rename = "on-success")]
    pub on_success: Option<StepRefs>,
    #[serde(default, rename = "on-failure")]
    pub on_failure: Option<StepRefs>,
    #[serde(default, rename = "on-return")]
    pub on_return: Option<HashMap<String, StepRefs>>,
    #[serde(default)]
    pub then: Vec<ChildRef>,
    #[serde(default)]
    pub meta: Meta,
}

// ── Step references ──

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum StepRefs {
    Single(StepRef),
    Multiple(Vec<StepRef>),
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum StepRef {
    Id(String),
    Inline(Box<Step>),
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum ChildRef {
    Id(String),
    Inline(Box<Step>),
}

// ── Meta ──

#[derive(Debug, Clone, Default, Deserialize)]
#[allow(dead_code)]
pub struct Meta {
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

// ── Action (tagged by "kind") ──

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Action {
    Shell {
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

// ── IO ──

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum IoLevel {
    Log,
    Info,
    Warn,
    Error,
}

// ── Git ──

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum GitOnConflict {
    #[default]
    Skip,
    Pull,
    Fail,
}

// ── FS ──

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FsOp {
    Create {
        path: PathSpec,
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
        path: PathSpec,
        #[serde(default)]
        recurse: bool,
    },
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum PathSpec {
    Single(String),
    Multiple(Vec<String>),
}

// ── Condition (if-exists / if-not-exists) ──

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

// ── Errors ──

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

// ── Parser ──

pub fn parse_config(path: &str, vars: &HashMap<String, String>) -> Result<Config, ConfigError> {
    let content = std::fs::read_to_string(path)?;
    let mut buf = Vec::new();
    io::Read::read_to_end(&mut json_comments::StripComments::new(content.as_bytes()), &mut buf)?;
    let mut json = String::from_utf8_lossy(&buf).into_owned();

    json = json.replace("\\{\\{", "\x00LBRACE\x00");

    // Built-in {{timestamp}} and {{timestamp:FORMAT}} variables
    let now = chrono::Local::now();
    // Replace {{timestamp:FORMAT}} first (greedy match)
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
    // Replace plain {{timestamp}} with default format
    let default_ts = now.format("%Y%m%dT%H%M%S").to_string();
    json = json.replace("{{timestamp}}", &default_ts);

    for (key, val) in vars {
        json = json.replace(&format!("{{{{{key}}}}}"), val);
    }

    if let Some(pos) = json.find("{{")
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

// ── Validation ──

fn validate_unique_ids(config: &Config) -> Result<(), ConfigError> {
    let mut seen = std::collections::HashSet::new();
    for step in &config.steps {
        collect_ids(step, &mut seen)?;
    }
    Ok(())
}

fn collect_ids(step: &Step, seen: &mut std::collections::HashSet<String>) -> Result<(), ConfigError> {
    if let Some(id) = &step.id
        && !seen.insert(id.clone())
    {
        return Err(ConfigError::DuplicateId(id.clone()));
    }
    for child in &step.then {
        if let ChildRef::Inline(s) = child { collect_ids(s, seen)?; }
    }
    visit_step_refs(step, &mut |sr| {
        if let StepRef::Inline(s) = sr { collect_ids(s, seen)?; }
        Ok(())
    })
}

/// Build a map of id → Step for reference resolution.
pub fn build_step_index(config: &Config) -> HashMap<String, Step> {
    let mut map = HashMap::new();
    for step in &config.steps { index_step(step, &mut map); }
    map
}

fn index_step(step: &Step, map: &mut HashMap<String, Step>) {
    if let Some(id) = &step.id { map.insert(id.clone(), step.clone()); }
    for child in &step.then {
        if let ChildRef::Inline(s) = child { index_step(s, map); }
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
        if let ChildRef::Inline(s) = child { collect_all_ids(s, ids); }
    }
    let _ = visit_step_refs(step, &mut |sr| {
        if let StepRef::Inline(s) = sr { collect_all_ids(s, ids); }
        Ok(())
    });
}

fn check_refs(step: &Step, ids: &std::collections::HashSet<String>) -> Result<(), ConfigError> {
    for child in &step.then {
        match child {
            ChildRef::Id(id) => { if !ids.contains(id) { return Err(ConfigError::UnknownRef(id.clone())); } }
            ChildRef::Inline(s) => check_refs(s, ids)?,
        }
    }
    visit_step_refs(step, &mut |sr| check_step_ref(sr, ids))
}

fn check_step_ref(sr: &StepRef, ids: &std::collections::HashSet<String>) -> Result<(), ConfigError> {
    match sr {
        StepRef::Id(id) => { if !ids.contains(id) { Err(ConfigError::UnknownRef(id.clone())) } else { Ok(()) } }
        StepRef::Inline(s) => check_refs(s, ids),
    }
}

/// Visit all StepRef values in a step's on-success, on-failure, on-return, and conditions.
fn visit_step_refs(step: &Step, f: &mut impl FnMut(&StepRef) -> Result<(), ConfigError>) -> Result<(), ConfigError> {
    if let Some(refs) = &step.on_success { visit_steprefs(refs, f)?; }
    if let Some(refs) = &step.on_failure { visit_steprefs(refs, f)?; }
    if let Some(map) = &step.on_return {
        for refs in map.values() { visit_steprefs(refs, f)?; }
    }
    if let Action::Fs { if_exists: Some(Condition::Execute { execute }), .. } = &step.action { f(execute)?; }
    if let Action::Fs { if_not_exists: Some(Condition::Execute { execute }), .. } = &step.action { f(execute)?; }
    Ok(())
}

fn visit_steprefs(refs: &StepRefs, f: &mut impl FnMut(&StepRef) -> Result<(), ConfigError>) -> Result<(), ConfigError> {
    match refs {
        StepRefs::Single(sr) => f(sr),
        StepRefs::Multiple(v) => { for sr in v { f(sr)?; } Ok(()) }
    }
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
        if let ChildRef::Inline(s) = child { check_markup(s)?; }
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
    fn parse_fs_create_single() {
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
                assert!(matches!(path, PathSpec::Single(p) if p == "~/projects/"));
                assert!(recurse);
                assert!(matches!(if_exists, Some(Condition::Action(ConditionAction::Skip))));
            }
            _ => panic!("expected Fs Create"),
        }
    }

    #[test]
    fn parse_fs_create_multiple() {
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
                assert!(matches!(path, PathSpec::Multiple(v) if v.len() == 2));
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
                "action": { "kind": "fs", "symlink": { "from": "~/.dotfiles/.bashrc", "to": "~/.bashrc" }, "if-exists": "overwrite" }
            }]
        }"#;
        let cfg: Config = serde_json::from_str(json).unwrap();
        match &cfg.steps[0].action {
            Action::Fs { op: FsOp::Symlink { from, to }, if_exists, .. } => {
                assert_eq!(from, "~/.dotfiles/.bashrc");
                assert_eq!(to, "~/.bashrc");
                assert!(matches!(if_exists, Some(Condition::Action(ConditionAction::Overwrite))));
            }
            _ => panic!("expected Fs Symlink"),
        }
    }

    #[test]
    fn parse_fs_copy_with_execute() {
        let json = r#"{
            "name": "test", "version": "1.0.0",
            "steps": [{
                "name": "copy",
                "action": { "kind": "fs", "copy": { "from": "a.txt", "to": "b.txt" }, "if-exists": { "execute": "handler-id" } }
            }]
        }"#;
        let cfg: Config = serde_json::from_str(json).unwrap();
        match &cfg.steps[0].action {
            Action::Fs { if_exists: Some(Condition::Execute { execute: StepRef::Id(id) }), .. } => {
                assert_eq!(id, "handler-id");
            }
            _ => panic!("expected Fs with execute condition"),
        }
    }

    #[test]
    fn parse_fs_delete() {
        let json = r#"{
            "name": "test", "version": "1.0.0",
            "steps": [{
                "name": "del",
                "action": { "kind": "fs", "delete": { "path": "~/.cache/old", "recurse": true }, "if-not-exists": "skip" }
            }]
        }"#;
        let cfg: Config = serde_json::from_str(json).unwrap();
        match &cfg.steps[0].action {
            Action::Fs { op: FsOp::Delete { path, recurse }, if_not_exists, .. } => {
                assert!(matches!(path, PathSpec::Single(p) if p == "~/.cache/old"));
                assert!(recurse);
                assert!(matches!(if_not_exists, Some(Condition::Action(ConditionAction::Skip))));
            }
            _ => panic!("expected Fs Delete"),
        }
    }

    #[test]
    fn parse_fs_move() {
        let json = r#"{
            "name": "test", "version": "1.0.0",
            "steps": [{
                "name": "mv",
                "action": { "kind": "fs", "move": { "from": "a", "to": "b" }, "if-not-exists": "skip" }
            }]
        }"#;
        let cfg: Config = serde_json::from_str(json).unwrap();
        match &cfg.steps[0].action {
            Action::Fs { op: FsOp::Move { from, to }, .. } => {
                assert_eq!(from, "a");
                assert_eq!(to, "b");
            }
            _ => panic!("expected Fs Move"),
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
        assert!(matches!(&cfg.steps[0].on_success, Some(StepRefs::Single(StepRef::Id(id))) if id == "next-step"));
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
        match &cfg.steps[0].on_success {
            Some(StepRefs::Multiple(v)) => assert_eq!(v.len(), 2),
            _ => panic!("expected Multiple"),
        }
    }

    #[test]
    fn parse_on_failure() {
        let json = r#"{
            "name": "test", "version": "1.0.0",
            "steps": [{
                "name": "run",
                "action": { "kind": "shell", "commands": ["echo"] },
                "on-failure": "error-handler"
            }]
        }"#;
        let cfg: Config = serde_json::from_str(json).unwrap();
        assert!(matches!(&cfg.steps[0].on_failure, Some(StepRefs::Single(StepRef::Id(id))) if id == "error-handler"));
    }

    #[test]
    fn parse_on_return() {
        let json = r#"{
            "name": "test", "version": "1.0.0",
            "steps": [{
                "name": "run",
                "action": { "kind": "shell", "commands": ["echo"] },
                "on-return": { "0": "success-step", "_": "fallback-step" }
            }]
        }"#;
        let cfg: Config = serde_json::from_str(json).unwrap();
        let map = cfg.steps[0].on_return.as_ref().unwrap();
        assert!(matches!(&map["0"], StepRefs::Single(StepRef::Id(id)) if id == "success-step"));
        assert!(matches!(&map["_"], StepRefs::Single(StepRef::Id(id)) if id == "fallback-step"));
    }

    #[test]
    fn parse_meta() {
        let json = r#"{
            "name": "test", "version": "1.0.0",
            "steps": [{
                "name": "run",
                "action": { "kind": "shell", "commands": ["echo"] },
                "meta": { "optional": true, "fallible": true, "silent": ["stdout", "stderr"], "retries": 3, "retry-delay": 2.0 }
            }]
        }"#;
        let cfg: Config = serde_json::from_str(json).unwrap();
        let meta = &cfg.steps[0].meta;
        assert!(meta.optional);
        assert!(meta.fallible);
        assert_eq!(meta.silent, vec![Silent::Stdout, Silent::Stderr]);
        assert_eq!(meta.retries, Some(3));
        assert_eq!(meta.retry_delay, Some(2.0));
    }

    #[test]
    fn parse_meta_sudo() {
        let json = r#"{
            "name": "test", "version": "1.0.0",
            "steps": [{
                "name": "run",
                "action": { "kind": "shell", "commands": ["apt install -y foo"] },
                "meta": { "sudo": true }
            }]
        }"#;
        let cfg: Config = serde_json::from_str(json).unwrap();
        assert!(cfg.steps[0].meta.sudo);
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
        assert!(matches!(&cfg.steps[0].then[0], ChildRef::Id(id) if id == "ref-id"));
        assert!(matches!(&cfg.steps[0].then[1], ChildRef::Inline(_)));
    }

    #[test]
    fn parse_global_retries() {
        let json = r#"{ "name": "test", "version": "1.0.0", "meta": { "retries": 5 }, "steps": [] }"#;
        let cfg: Config = serde_json::from_str(json).unwrap();
        assert_eq!(cfg.meta.retries, Some(5));
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
        let dir = std::env::temp_dir();
        let path = dir.join("rig_dup_test.json");
        std::fs::write(&path, json).unwrap();
        assert!(matches!(parse_config(path.to_str().unwrap(), &HashMap::new()), Err(ConfigError::DuplicateId(_))));
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn unknown_then_ref_rejected() {
        let json = r#"{
            "name": "test", "version": "1.0.0",
            "steps": [{
                "name": "parent",
                "action": { "kind": "shell", "commands": ["echo"] },
                "then": ["nonexistent"]
            }]
        }"#;
        let dir = std::env::temp_dir();
        let path = dir.join("rig_unknownref_test.json");
        std::fs::write(&path, json).unwrap();
        assert!(matches!(parse_config(path.to_str().unwrap(), &HashMap::new()), Err(ConfigError::UnknownRef(_))));
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn unknown_on_success_ref_rejected() {
        let json = r#"{
            "name": "test", "version": "1.0.0",
            "steps": [{
                "name": "run",
                "action": { "kind": "shell", "commands": ["echo"] },
                "on-success": "ghost"
            }]
        }"#;
        let dir = std::env::temp_dir();
        let path = dir.join("rig_unknownsuccess_test.json");
        std::fs::write(&path, json).unwrap();
        assert!(matches!(parse_config(path.to_str().unwrap(), &HashMap::new()), Err(ConfigError::UnknownRef(_))));
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
        let dir = std::env::temp_dir();
        let path = dir.join("rig_validref_test.json");
        std::fs::write(&path, json).unwrap();
        assert!(parse_config(path.to_str().unwrap(), &HashMap::new()).is_ok());
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn cycle_allowed_at_parse_time() {
        let json = r#"{
            "name": "test", "version": "1.0.0",
            "steps": [
                { "id": "a", "name": "a", "action": { "kind": "shell", "commands": ["echo"] }, "then": ["b"] },
                { "id": "b", "name": "b", "action": { "kind": "shell", "commands": ["echo"] }, "then": ["a"] }
            ]
        }"#;
        let dir = std::env::temp_dir();
        let path = dir.join("rig_cycle_test.json");
        std::fs::write(&path, json).unwrap();
        assert!(parse_config(path.to_str().unwrap(), &HashMap::new()).is_ok());
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn step_index_built() {
        let json = r#"{
            "name": "test", "version": "1.0.0",
            "steps": [
                { "id": "a", "name": "step a", "action": { "kind": "shell", "commands": ["echo a"] } },
                { "name": "no id", "action": { "kind": "shell", "commands": ["echo b"] } }
            ]
        }"#;
        let cfg: Config = serde_json::from_str(json).unwrap();
        let idx = build_step_index(&cfg);
        assert_eq!(idx.len(), 1);
        assert_eq!(idx["a"].name, "step a");
    }

    #[test]
    fn var_substitution() {
        let json = r#"{
            "name": "{{project}}", "version": "1.0.0",
            "steps": [{
                "name": "run",
                "action": { "kind": "shell", "commands": ["echo {{greeting}}"] }
            }]
        }"#;
        let dir = std::env::temp_dir();
        let path = dir.join("rig_var_test.json");
        std::fs::write(&path, json).unwrap();
        let vars = HashMap::from([("project".into(), "my-app".into()), ("greeting".into(), "hello".into())]);
        let cfg = parse_config(path.to_str().unwrap(), &vars).unwrap();
        assert_eq!(cfg.name, "my-app");
        match &cfg.steps[0].action {
            Action::Shell { commands, .. } => assert_eq!(commands[0], "echo hello"),
            _ => panic!("expected Shell"),
        }
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn undefined_var_rejected() {
        let json = r#"{ "name": "{{missing}}", "version": "1.0.0", "steps": [] }"#;
        let dir = std::env::temp_dir();
        let path = dir.join("rig_undef_test.json");
        std::fs::write(&path, json).unwrap();
        assert!(matches!(
            parse_config(path.to_str().unwrap(), &HashMap::new()),
            Err(ConfigError::UndefinedVar(v)) if v == "missing"
        ));
        std::fs::remove_file(path).ok();
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
        match &cfg.steps[0].action {
            Action::Io { level, message, markup } => {
                assert_eq!(*level, IoLevel::Info);
                assert_eq!(message, "hello");
                assert!(!markup);
            }
            _ => panic!("expected Io"),
        }
    }

    #[test]
    fn parse_io_with_markup() {
        let json = r#"{
            "name": "test", "version": "1.0.0",
            "steps": [{
                "name": "log",
                "action": { "kind": "io", "level": "warn", "message": "<fy>warning!</f>", "markup": true }
            }]
        }"#;
        let cfg: Config = serde_json::from_str(json).unwrap();
        match &cfg.steps[0].action {
            Action::Io { level, markup, .. } => {
                assert_eq!(*level, IoLevel::Warn);
                assert!(markup);
            }
            _ => panic!("expected Io"),
        }
    }

    #[test]
    fn parse_top_level_meta() {
        let json = r#"{
            "name": "test", "version": "1.0.0",
            "meta": { "log": "/tmp/test.log", "silent": ["stdout"], "sudo": true, "retries": 2, "retry-delay": 1.5 },
            "steps": []
        }"#;
        let cfg: Config = serde_json::from_str(json).unwrap();
        assert_eq!(cfg.meta.log.as_deref(), Some("/tmp/test.log"));
        assert_eq!(cfg.meta.silent, vec![Silent::Stdout]);
        assert!(cfg.meta.sudo);
        assert_eq!(cfg.meta.retries, Some(2));
        assert_eq!(cfg.meta.retry_delay, Some(1.5));
    }

    #[test]
    fn timestamp_substituted() {
        let json = r#"{ "name": "run-{{timestamp}}", "version": "1.0.0", "steps": [] }"#;
        let dir = std::env::temp_dir();
        let path = dir.join("rig_ts_test.json");
        std::fs::write(&path, json).unwrap();
        let cfg = parse_config(path.to_str().unwrap(), &HashMap::new()).unwrap();
        assert!(!cfg.name.contains("{{timestamp}}"));
        assert!(cfg.name.starts_with("run-20"));
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn timestamp_custom_format() {
        let json = r#"{ "name": "run-{{timestamp:%Y-%m-%d}}", "version": "1.0.0", "steps": [] }"#;
        let dir = std::env::temp_dir();
        let path = dir.join("rig_tsfmt_test.json");
        std::fs::write(&path, json).unwrap();
        let cfg = parse_config(path.to_str().unwrap(), &HashMap::new()).unwrap();
        // Should be like "run-2026-05-04"
        assert!(cfg.name.starts_with("run-20"));
        assert!(cfg.name.contains('-'));
        assert_eq!(cfg.name.len(), "run-2026-05-04".len());
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn valid_markup_accepted() {
        let json = r#"{
            "name": "test", "version": "1.0.0",
            "steps": [{
                "name": "log",
                "action": { "kind": "io", "level": "info", "message": "<fg>hello</f>", "markup": true }
            }]
        }"#;
        let dir = std::env::temp_dir();
        let path = dir.join("rig_markup_ok_test.json");
        std::fs::write(&path, json).unwrap();
        assert!(parse_config(path.to_str().unwrap(), &HashMap::new()).is_ok());
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn invalid_markup_rejected() {
        let json = r#"{
            "name": "test", "version": "1.0.0",
            "steps": [{
                "name": "bad",
                "action": { "kind": "io", "level": "info", "message": "<invalid_tag>oops", "markup": true }
            }]
        }"#;
        let dir = std::env::temp_dir();
        let path = dir.join("rig_markup_bad_test.json");
        std::fs::write(&path, json).unwrap();
        assert!(matches!(
            parse_config(path.to_str().unwrap(), &HashMap::new()),
            Err(ConfigError::InvalidMarkup(..))
        ));
        std::fs::remove_file(path).ok();
    }
}
