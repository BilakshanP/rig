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
    #[serde(default, rename = "max-retries")]
    pub max_retries: Option<u32>,
    pub steps: Vec<Step>,
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
    #[serde(default)]
    pub children: Vec<ChildRef>,
    #[serde(default)]
    pub meta: Meta,
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
    pub silent: Vec<Silent>,
    #[serde(default, rename = "max-retries")]
    pub max_retries: Option<u32>,
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
        #[serde(default, rename = "on-return")]
        on_return: Option<HashMap<String, StepRef>>,
    },
    Git {
        repo: String,
        dest: String,
        #[serde(default, rename = "if-exists")]
        if_exists: GitIfExists,
    },
    Fs {
        action: FsAction,
        #[serde(default)]
        recurse: bool,
        #[serde(flatten)]
        target: FsTarget,
        #[serde(default, rename = "if-exists")]
        if_exists: Option<Condition>,
        #[serde(default, rename = "if-not-exists")]
        if_not_exists: Option<Condition>,
    },
}

// ── Step references (id string or inline step) ──

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum StepRef {
    Id(String),
    Inline(Box<Step>),
}

// ── Git ──

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum GitIfExists {
    #[default]
    Skip,
    Pull,
    Fail,
}

// ── FS ──

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum FsAction {
    Create,
    Symlink,
    Copy,
    Move,
    Delete,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum FsTarget {
    FromTo {
        from: String,
        to: String,
    },
    Path {
        path: PathSpec,
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
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "failed to read config: {e}"),
            Self::Parse(e) => write!(f, "failed to parse config: {e}"),
            Self::DuplicateId(id) => write!(f, "duplicate step id: {id}"),
            Self::UnknownRef(id) => write!(f, "unknown step reference: {id}"),
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

pub fn parse_config(path: &str) -> Result<Config, ConfigError> {
    let content = std::fs::read_to_string(path)?;
    let stripped = json_comments::StripComments::new(content.as_bytes());
    let config: Config = serde_json::from_reader(stripped)?;
    validate_unique_ids(&config)?;
    validate_refs(&config)?;
    Ok(config)
}

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
    for child in &step.children {
        if let ChildRef::Inline(s) = child {
            collect_ids(s, seen)?;
        }
    }
    Ok(())
}

/// Build a map of id → Step for reference resolution.
pub fn build_step_index(config: &Config) -> HashMap<String, Step> {
    let mut map = HashMap::new();
    for step in &config.steps {
        index_step(step, &mut map);
    }
    map
}

fn index_step(step: &Step, map: &mut HashMap<String, Step>) {
    if let Some(id) = &step.id {
        map.insert(id.clone(), step.clone());
    }
    for child in &step.children {
        if let ChildRef::Inline(s) = child {
            index_step(s, map);
        }
    }
}

fn validate_refs(config: &Config) -> Result<(), ConfigError> {
    let mut ids = std::collections::HashSet::new();
    for step in &config.steps {
        collect_all_ids(step, &mut ids);
    }
    for step in &config.steps {
        check_refs(step, &ids)?;
    }
    Ok(())
}

fn collect_all_ids(step: &Step, ids: &mut std::collections::HashSet<String>) {
    if let Some(id) = &step.id {
        ids.insert(id.clone());
    }
    for child in &step.children {
        if let ChildRef::Inline(s) = child {
            collect_all_ids(s, ids);
        }
    }
    // Also collect IDs from inline steps in on-return/conditions
    check_inline_ids(&step.action, ids);
}

fn check_inline_ids(action: &Action, ids: &mut std::collections::HashSet<String>) {
    match action {
        Action::Shell { on_return: Some(map), .. } => {
            for sr in map.values() {
                if let StepRef::Inline(s) = sr { collect_all_ids(s, ids); }
            }
        }
        Action::Fs { if_exists: Some(Condition::Execute { execute }), .. } |
        Action::Fs { if_not_exists: Some(Condition::Execute { execute }), .. } => {
            if let StepRef::Inline(s) = execute { collect_all_ids(s, ids); }
        }
        _ => {}
    }
}

fn check_refs(step: &Step, ids: &std::collections::HashSet<String>) -> Result<(), ConfigError> {
    // Check children refs
    for child in &step.children {
        match child {
            ChildRef::Id(id) => {
                if !ids.contains(id) { return Err(ConfigError::UnknownRef(id.clone())); }
            }
            ChildRef::Inline(s) => check_refs(s, ids)?,
        }
    }
    // Check action refs
    check_action_refs(&step.action, ids)
}

fn check_action_refs(action: &Action, ids: &std::collections::HashSet<String>) -> Result<(), ConfigError> {
    match action {
        Action::Shell { on_return: Some(map), .. } => {
            for sr in map.values() {
                check_step_ref(sr, ids)?;
            }
        }
        Action::Fs { if_exists, if_not_exists, .. } => {
            if let Some(Condition::Execute { execute }) = if_exists { check_step_ref(execute, ids)?; }
            if let Some(Condition::Execute { execute }) = if_not_exists { check_step_ref(execute, ids)?; }
        }
        _ => {}
    }
    Ok(())
}

fn check_step_ref(sr: &StepRef, ids: &std::collections::HashSet<String>) -> Result<(), ConfigError> {
    match sr {
        StepRef::Id(id) => {
            if !ids.contains(id) { Err(ConfigError::UnknownRef(id.clone())) } else { Ok(()) }
        }
        StepRef::Inline(s) => check_refs(s, ids),
    }
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
                "action": {
                    "kind": "shell",
                    "commands": ["echo hi"],
                    "dir": "~",
                    "env": {"FOO": "bar"}
                }
            }]
        }"#;
        let cfg: Config = serde_json::from_str(json).unwrap();
        match &cfg.steps[0].action {
            Action::Shell { commands, dir, env, .. } => {
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
                "action": {
                    "kind": "git",
                    "repo": "https://github.com/user/repo.git",
                    "dest": "~/.dotfiles",
                    "if-exists": "pull"
                }
            }]
        }"#;
        let cfg: Config = serde_json::from_str(json).unwrap();
        match &cfg.steps[0].action {
            Action::Git { repo, dest, if_exists } => {
                assert_eq!(repo, "https://github.com/user/repo.git");
                assert_eq!(dest, "~/.dotfiles");
                assert_eq!(*if_exists, GitIfExists::Pull);
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
                "action": {
                    "kind": "fs",
                    "action": "create",
                    "path": "~/projects/",
                    "recurse": true,
                    "if-exists": "skip"
                }
            }]
        }"#;
        let cfg: Config = serde_json::from_str(json).unwrap();
        match &cfg.steps[0].action {
            Action::Fs { action, recurse, target, if_exists, .. } => {
                assert_eq!(*action, FsAction::Create);
                assert!(recurse);
                assert!(matches!(target, FsTarget::Path { path: PathSpec::Single(p) } if p == "~/projects/"));
                assert!(matches!(if_exists, Some(Condition::Action(ConditionAction::Skip))));
            }
            _ => panic!("expected Fs"),
        }
    }

    #[test]
    fn parse_fs_create_multiple() {
        let json = r#"{
            "name": "test", "version": "1.0.0",
            "steps": [{
                "name": "create",
                "action": {
                    "kind": "fs",
                    "action": "create",
                    "path": ["~/a/", "~/b/"],
                    "recurse": true
                }
            }]
        }"#;
        let cfg: Config = serde_json::from_str(json).unwrap();
        match &cfg.steps[0].action {
            Action::Fs { target, .. } => {
                assert!(matches!(target, FsTarget::Path { path: PathSpec::Multiple(v) } if v.len() == 2));
            }
            _ => panic!("expected Fs"),
        }
    }

    #[test]
    fn parse_fs_symlink() {
        let json = r#"{
            "name": "test", "version": "1.0.0",
            "steps": [{
                "name": "link",
                "action": {
                    "kind": "fs",
                    "action": "symlink",
                    "from": "~/.dotfiles/.bashrc",
                    "to": "~/.bashrc",
                    "if-exists": "overwrite"
                }
            }]
        }"#;
        let cfg: Config = serde_json::from_str(json).unwrap();
        match &cfg.steps[0].action {
            Action::Fs { action, target, if_exists, .. } => {
                assert_eq!(*action, FsAction::Symlink);
                assert!(matches!(target, FsTarget::FromTo { from, to } if from == "~/.dotfiles/.bashrc" && to == "~/.bashrc"));
                assert!(matches!(if_exists, Some(Condition::Action(ConditionAction::Overwrite))));
            }
            _ => panic!("expected Fs"),
        }
    }

    #[test]
    fn parse_fs_copy_with_execute() {
        let json = r#"{
            "name": "test", "version": "1.0.0",
            "steps": [{
                "name": "copy",
                "action": {
                    "kind": "fs",
                    "action": "copy",
                    "from": "a.txt",
                    "to": "b.txt",
                    "if-exists": { "execute": "handler-id" }
                }
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
    fn parse_on_return() {
        let json = r#"{
            "name": "test", "version": "1.0.0",
            "steps": [{
                "name": "run",
                "action": {
                    "kind": "shell",
                    "commands": ["echo hi"],
                    "on-return": {
                        "0": "success-step",
                        "_": "fallback-step"
                    }
                }
            }]
        }"#;
        let cfg: Config = serde_json::from_str(json).unwrap();
        match &cfg.steps[0].action {
            Action::Shell { on_return: Some(map), .. } => {
                assert!(matches!(&map["0"], StepRef::Id(id) if id == "success-step"));
                assert!(matches!(&map["_"], StepRef::Id(id) if id == "fallback-step"));
            }
            _ => panic!("expected Shell with on-return"),
        }
    }

    #[test]
    fn parse_meta() {
        let json = r#"{
            "name": "test", "version": "1.0.0",
            "steps": [{
                "name": "run",
                "action": { "kind": "shell", "commands": ["echo"] },
                "meta": { "optional": true, "fallible": true, "silent": ["stdout", "stderr"] }
            }]
        }"#;
        let cfg: Config = serde_json::from_str(json).unwrap();
        let meta = &cfg.steps[0].meta;
        assert!(meta.optional);
        assert!(meta.fallible);
        assert_eq!(meta.silent, vec![Silent::Stdout, Silent::Stderr]);
    }

    #[test]
    fn parse_children_mixed() {
        let json = r#"{
            "name": "test", "version": "1.0.0",
            "steps": [{
                "name": "parent",
                "action": { "kind": "shell", "commands": ["echo"] },
                "children": [
                    "ref-id",
                    { "name": "inline", "action": { "kind": "shell", "commands": ["echo inline"] } }
                ]
            }]
        }"#;
        let cfg: Config = serde_json::from_str(json).unwrap();
        assert_eq!(cfg.steps[0].children.len(), 2);
        assert!(matches!(&cfg.steps[0].children[0], ChildRef::Id(id) if id == "ref-id"));
        assert!(matches!(&cfg.steps[0].children[1], ChildRef::Inline(_)));
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
        let path = dir.join("devsetup_dup_test.json");
        std::fs::write(&path, json).unwrap();
        assert!(matches!(parse_config(path.to_str().unwrap()), Err(ConfigError::DuplicateId(_))));
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn unknown_child_ref_rejected() {
        let json = r#"{
            "name": "test", "version": "1.0.0",
            "steps": [{
                "name": "parent",
                "action": { "kind": "shell", "commands": ["echo"] },
                "children": ["nonexistent"]
            }]
        }"#;
        let dir = std::env::temp_dir();
        let path = dir.join("devsetup_unknownref_test.json");
        std::fs::write(&path, json).unwrap();
        assert!(matches!(parse_config(path.to_str().unwrap()), Err(ConfigError::UnknownRef(_))));
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn unknown_on_return_ref_rejected() {
        let json = r#"{
            "name": "test", "version": "1.0.0",
            "steps": [{
                "name": "run",
                "action": { "kind": "shell", "commands": ["echo"], "on-return": { "0": "ghost" } }
            }]
        }"#;
        let dir = std::env::temp_dir();
        let path = dir.join("devsetup_unknownor_test.json");
        std::fs::write(&path, json).unwrap();
        assert!(matches!(parse_config(path.to_str().unwrap()), Err(ConfigError::UnknownRef(_))));
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn valid_refs_accepted() {
        let json = r#"{
            "name": "test", "version": "1.0.0",
            "steps": [
                { "id": "a", "name": "a", "action": { "kind": "shell", "commands": ["echo"] } },
                { "name": "b", "action": { "kind": "shell", "commands": ["echo"] }, "children": ["a"] }
            ]
        }"#;
        let dir = std::env::temp_dir();
        let path = dir.join("devsetup_validref_test.json");
        std::fs::write(&path, json).unwrap();
        assert!(parse_config(path.to_str().unwrap()).is_ok());
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn cycle_allowed_at_parse_time() {
        let json = r#"{
            "name": "test", "version": "1.0.0",
            "steps": [
                { "id": "a", "name": "a", "action": { "kind": "shell", "commands": ["echo"] }, "children": ["b"] },
                { "id": "b", "name": "b", "action": { "kind": "shell", "commands": ["echo"] }, "children": ["a"] }
            ]
        }"#;
        let dir = std::env::temp_dir();
        let path = dir.join("devsetup_cycle_test.json");
        std::fs::write(&path, json).unwrap();
        assert!(parse_config(path.to_str().unwrap()).is_ok());
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
}
