use crate::config::*;
use crate::path::expand_tilde;
use std::collections::HashMap;
use std::fmt;
use std::process::Command;

#[derive(Debug)]
pub enum ExecError {
    Command(String),
    Io(std::io::Error),
    StepNotFound(String),
    MaxRetriesExceeded(String),
}

impl fmt::Display for ExecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Command(msg) => write!(f, "{msg}"),
            Self::Io(e) => write!(f, "{e}"),
            Self::StepNotFound(id) => write!(f, "step not found: {id}"),
            Self::MaxRetriesExceeded(name) => write!(f, "max retries exceeded for step: {name}"),
        }
    }
}

impl From<std::io::Error> for ExecError {
    fn from(e: std::io::Error) -> Self { Self::Io(e) }
}

use std::cell::RefCell;

pub struct Runner {
    pub index: HashMap<String, Step>,
    pub dry_run: bool,
    pub verbose: bool,
    pub global_max_retries: Option<u32>,
    entry_counts: RefCell<HashMap<String, u32>>,
}

impl Runner {
    pub fn new(index: HashMap<String, Step>, dry_run: bool, verbose: bool, global_max_retries: Option<u32>) -> Self {
        Self { index, dry_run, verbose, global_max_retries, entry_counts: RefCell::new(HashMap::new()) }
    }

    pub fn run_steps(&self, steps: &[Step]) -> Result<(), ExecError> {
        for step in steps {
            if step.meta.optional {
                continue;
            }
            self.run_step(step, 0)?;
        }
        Ok(())
    }

    pub fn run_step(&self, step: &Step, depth: usize) -> Result<(), ExecError> {
        let indent = "  ".repeat(depth);

        // Enforce max retries for steps with an ID
        if let Some(id) = &step.id {
            let mut counts = self.entry_counts.borrow_mut();
            let count = counts.entry(id.clone()).or_insert(0);
            *count += 1;
            let max = step.meta.max_retries.or(self.global_max_retries);
            let allowed = 1 + max.unwrap_or(0); // first run + retries
            if *count > allowed {
                return Err(ExecError::MaxRetriesExceeded(step.name.clone()));
            }
            // Sleep before retry (not on first entry)
            if *count > 1
                && let Some(delay) = step.meta.retry_delay
            {
                if !self.dry_run {
                    println!("{indent}  ⏳ retrying in {delay}s...");
                    std::thread::sleep(std::time::Duration::from_secs_f64(delay));
                } else {
                    println!("{indent}  [dry-run] would sleep {delay}s before retry");
                }
            }
        }

        println!("{indent}→ {}", step.name);

        let result = match &step.action {
            Action::Shell { commands, dir, env, on_return } => {
                self.exec_shell(commands, dir.as_deref(), env.as_ref(), on_return.as_ref(), &step.meta, &indent, depth)
            }
            Action::Git { repo, dest, if_exists } => {
                self.exec_git(repo, dest, if_exists, &step.meta, &indent)
            }
            Action::Fs { action, recurse, target, if_exists, if_not_exists } => {
                self.exec_fs(action, *recurse, target, if_exists.as_ref(), if_not_exists.as_ref(), &indent)
            }
        };

        match result {
            Ok(()) => {}
            Err(e) if step.meta.fallible => {
                println!("{indent}  ⚠ failed (fallible): {e}");
                return Ok(()); // don't run children
            }
            Err(e) => return Err(e),
        }

        // Run children after successful action
        for child in &step.children {
            self.run_child(child, depth + 1)?;
        }
        Ok(())
    }

    fn run_child(&self, child: &ChildRef, depth: usize) -> Result<(), ExecError> {
        match child {
            ChildRef::Id(id) => self.run_ref(id, depth),
            ChildRef::Inline(step) => self.run_step(step, depth),
        }
    }

    fn run_ref(&self, id: &str, depth: usize) -> Result<(), ExecError> {
        let step = self.index.get(id).ok_or_else(|| ExecError::StepNotFound(id.into()))?;
        self.run_step(step, depth)
    }

    fn run_step_ref(&self, sr: &StepRef, depth: usize) -> Result<(), ExecError> {
        match sr {
            StepRef::Id(id) => self.run_ref(id, depth),
            StepRef::Inline(step) => self.run_step(step, depth),
        }
    }

    fn maybe_print(&self, stdout: &[u8], stderr: &[u8], meta: &Meta) {
        let show_out = !meta.silent.contains(&Silent::Stdout) || self.verbose;
        let show_err = !meta.silent.contains(&Silent::Stderr) || self.verbose;
        if show_out && !stdout.is_empty() { print!("{}", String::from_utf8_lossy(stdout)); }
        if show_err && !stderr.is_empty() { eprint!("{}", String::from_utf8_lossy(stderr)); }
    }

    // ── Shell ──

    #[allow(clippy::too_many_arguments)]
    fn exec_shell(
        &self,
        commands: &[String],
        dir: Option<&str>,
        env: Option<&HashMap<String, String>>,
        on_return: Option<&HashMap<String, StepRef>>,
        meta: &Meta,
        indent: &str,
        depth: usize,
    ) -> Result<(), ExecError> {
        for cmd in commands {
            if self.dry_run {
                println!("{indent}  [dry-run] sh -c {cmd:?}");
                if let Some(d) = dir { println!("{indent}    dir: {d}"); }
                if let Some(e) = env { println!("{indent}    env: {e:?}"); }
                continue;
            }
            let mut proc = Command::new("sh");
            proc.arg("-c").arg(cmd);
            if let Some(d) = dir { proc.current_dir(expand_tilde(d)); }
            if let Some(e) = env { proc.envs(e); }
            let output = proc.output()?;
            let code = output.status.code().unwrap_or(-1);
            self.maybe_print(&output.stdout, &output.stderr, meta);

            if let Some(map) = on_return {
                let key = code.to_string();
                if let Some(sr) = map.get(&key).or_else(|| map.get("_")) {
                    self.run_step_ref(sr, depth + 1)?;
                    // After on-return dispatch, continue (children will run in run_step)
                    // but don't fail even if code was non-zero — on-return handled it
                    return Ok(());
                }
            }
            if !output.status.success() {
                return Err(ExecError::Command(format!("command failed (exit {code}): {cmd}")));
            }
        }
        Ok(())
    }

    // ── Git ──

    fn exec_git(&self, repo: &str, dest: &str, if_exists: &GitIfExists, meta: &Meta, indent: &str) -> Result<(), ExecError> {
        let dest_path = expand_tilde(dest);
        let exists = dest_path.exists();

        if self.dry_run {
            if exists {
                println!("{indent}  [dry-run] dest {} exists → {if_exists:?}", dest_path.display());
            } else {
                println!("{indent}  [dry-run] git clone {repo} {}", dest_path.display());
            }
            return Ok(());
        }

        if !exists {
            let output = Command::new("git")
                .args(["clone", repo, &dest_path.to_string_lossy()])
                .output()?;
            self.maybe_print(&output.stdout, &output.stderr, meta);
            if !output.status.success() {
                return Err(ExecError::Command(format!("git clone failed ({})", output.status)));
            }
            return Ok(());
        }

        match if_exists {
            GitIfExists::Skip => println!("{indent}  skipped (exists): {}", dest_path.display()),
            GitIfExists::Pull => {
                let output = Command::new("git")
                    .args(["-C", &dest_path.to_string_lossy(), "pull"])
                    .output()?;
                self.maybe_print(&output.stdout, &output.stderr, meta);
                if !output.status.success() {
                    return Err(ExecError::Command(format!("git pull failed ({})", output.status)));
                }
            }
            GitIfExists::Fail => {
                return Err(ExecError::Command(format!("dest already exists: {}", dest_path.display())));
            }
        }
        Ok(())
    }

    // ── FS ──

    fn exec_fs(
        &self,
        action: &FsAction,
        recurse: bool,
        target: &FsTarget,
        if_exists: Option<&Condition>,
        if_not_exists: Option<&Condition>,
        indent: &str,
    ) -> Result<(), ExecError> {
        match action {
            FsAction::Create => self.fs_create(target, recurse, if_exists, indent),
            FsAction::Symlink => self.fs_symlink(target, if_exists, indent),
            FsAction::Copy => self.fs_copy(target, if_exists, if_not_exists, indent),
            FsAction::Move => self.fs_move(target, if_exists, if_not_exists, indent),
            FsAction::Delete => self.fs_delete(target, recurse, if_not_exists, indent),
        }
    }

    fn fs_create(&self, target: &FsTarget, recurse: bool, if_exists: Option<&Condition>, indent: &str) -> Result<(), ExecError> {
        let paths = match target {
            FsTarget::Path { path } => path_list(path),
            _ => return Err(ExecError::Command("create requires 'path' field".into())),
        };
        for p in paths {
            let full = expand_tilde(&p);
            let is_dir = p.ends_with('/');
            if self.dry_run {
                let kind = if is_dir { "dir" } else { "file" };
                println!("{indent}  [dry-run] create {kind}: {}", full.display());
                continue;
            }
            if full.exists() {
                match self.handle_condition(if_exists, indent)? {
                    CondResult::Skip => continue,
                    CondResult::Proceed => {} // overwrite
                    CondResult::Panic => return Err(ExecError::Command(format!("already exists: {}", full.display()))),
                }
            }
            if is_dir {
                if recurse { std::fs::create_dir_all(&full)?; } else { std::fs::create_dir(&full)?; }
            } else {
                if recurse && let Some(parent) = full.parent() { std::fs::create_dir_all(parent)?; }
                std::fs::File::create(&full)?;
            }
            println!("{indent}  created: {}", full.display());
        }
        Ok(())
    }

    fn fs_symlink(&self, target: &FsTarget, if_exists: Option<&Condition>, indent: &str) -> Result<(), ExecError> {
        let (from, to) = match target {
            FsTarget::FromTo { from, to } => (from.as_str(), to.as_str()),
            _ => return Err(ExecError::Command("symlink requires 'from' and 'to' fields".into())),
        };
        let src = expand_tilde(from);
        let dst = expand_tilde(to);
        if self.dry_run {
            println!("{indent}  [dry-run] symlink {} → {}", src.display(), dst.display());
            return Ok(());
        }
        if dst.exists() || dst.is_symlink() {
            match self.handle_condition(if_exists, indent)? {
                CondResult::Skip => return Ok(()),
                CondResult::Proceed => { std::fs::remove_file(&dst).or_else(|_| std::fs::remove_dir_all(&dst))?; }
                CondResult::Panic => return Err(ExecError::Command(format!("already exists: {}", dst.display()))),
            }
        }
        std::os::unix::fs::symlink(&src, &dst)?;
        println!("{indent}  symlinked {} → {}", src.display(), dst.display());
        Ok(())
    }

    fn fs_copy(&self, target: &FsTarget, if_exists: Option<&Condition>, if_not_exists: Option<&Condition>, indent: &str) -> Result<(), ExecError> {
        let (from, to) = match target {
            FsTarget::FromTo { from, to } => (from.as_str(), to.as_str()),
            _ => return Err(ExecError::Command("copy requires 'from' and 'to' fields".into())),
        };
        let src = expand_tilde(from);
        let dst = expand_tilde(to);
        if self.dry_run {
            println!("{indent}  [dry-run] copy {} → {}", src.display(), dst.display());
            return Ok(());
        }
        if !src.exists() {
            return self.handle_not_exists(if_not_exists, &src, indent);
        }
        if dst.exists() {
            match self.handle_condition(if_exists, indent)? {
                CondResult::Skip => return Ok(()),
                CondResult::Proceed => {}
                CondResult::Panic => return Err(ExecError::Command(format!("already exists: {}", dst.display()))),
            }
        }
        std::fs::copy(&src, &dst)?;
        println!("{indent}  copied {} → {}", src.display(), dst.display());
        Ok(())
    }

    fn fs_move(&self, target: &FsTarget, if_exists: Option<&Condition>, if_not_exists: Option<&Condition>, indent: &str) -> Result<(), ExecError> {
        let (from, to) = match target {
            FsTarget::FromTo { from, to } => (from.as_str(), to.as_str()),
            _ => return Err(ExecError::Command("move requires 'from' and 'to' fields".into())),
        };
        let src = expand_tilde(from);
        let dst = expand_tilde(to);
        if self.dry_run {
            println!("{indent}  [dry-run] move {} → {}", src.display(), dst.display());
            return Ok(());
        }
        if !src.exists() {
            return self.handle_not_exists(if_not_exists, &src, indent);
        }
        if dst.exists() {
            match self.handle_condition(if_exists, indent)? {
                CondResult::Skip => return Ok(()),
                CondResult::Proceed => { std::fs::remove_file(&dst).or_else(|_| std::fs::remove_dir_all(&dst))?; }
                CondResult::Panic => return Err(ExecError::Command(format!("already exists: {}", dst.display()))),
            }
        }
        std::fs::rename(&src, &dst)?;
        println!("{indent}  moved {} → {}", src.display(), dst.display());
        Ok(())
    }

    fn fs_delete(&self, target: &FsTarget, recurse: bool, if_not_exists: Option<&Condition>, indent: &str) -> Result<(), ExecError> {
        let paths = match target {
            FsTarget::Path { path } => path_list(path),
            _ => return Err(ExecError::Command("delete requires 'path' field".into())),
        };
        for p in paths {
            let full = expand_tilde(&p);
            if self.dry_run {
                println!("{indent}  [dry-run] delete: {}", full.display());
                continue;
            }
            if !full.exists() {
                self.handle_not_exists(if_not_exists, &full, indent)?;
                continue;
            }
            if full.is_dir() {
                if recurse { std::fs::remove_dir_all(&full)?; } else { std::fs::remove_dir(&full)?; }
            } else {
                std::fs::remove_file(&full)?;
            }
            println!("{indent}  deleted: {}", full.display());
        }
        Ok(())
    }

    // ── Condition helpers ──

    fn handle_condition(&self, cond: Option<&Condition>, _indent: &str) -> Result<CondResult, ExecError> {
        match cond {
            None | Some(Condition::Action(ConditionAction::Panic)) => Ok(CondResult::Panic),
            Some(Condition::Action(ConditionAction::Skip)) => Ok(CondResult::Skip),
            Some(Condition::Action(ConditionAction::Overwrite)) => Ok(CondResult::Proceed),
            Some(Condition::Action(ConditionAction::Append)) => Ok(CondResult::Proceed), // caller handles append semantics
            Some(Condition::Execute { execute }) => {
                self.run_step_ref(execute, 0)?;
                Ok(CondResult::Skip) // after executing handler, skip the original action
            }
        }
    }

    fn handle_not_exists(&self, cond: Option<&Condition>, path: &std::path::Path, _indent: &str) -> Result<(), ExecError> {
        match cond {
            None | Some(Condition::Action(ConditionAction::Panic)) => {
                Err(ExecError::Command(format!("does not exist: {}", path.display())))
            }
            Some(Condition::Action(ConditionAction::Skip)) => Ok(()),
            Some(Condition::Execute { execute }) => self.run_step_ref(execute, 0),
            _ => Err(ExecError::Command(format!("does not exist: {}", path.display()))),
        }
    }

    // ── Dry-run audit ──

    pub fn dry_run_audit(&self, steps: &[Step]) {
        let optional_count = steps.iter().filter(|s| s.meta.optional).count();
        let fallible_count = steps.iter().filter(|s| s.meta.fallible).count();
        println!("[dry-run] ({} steps, {} optional, {} fallible)\n", steps.len(), optional_count, fallible_count);
        for step in steps {
            self.audit_step(step, 0);
        }
        println!("Summary: {} steps ({} optional, {} fallible)", steps.len(), optional_count, fallible_count);
    }

    fn audit_step(&self, step: &Step, depth: usize) {
        let indent = "  ".repeat(depth);
        // Header: → Name (id: xxx) [flags...]
        let mut header = format!("{indent}→ {}", step.name);
        if let Some(id) = &step.id { header.push_str(&format!(" (id: {id})")); }
        let mut flags = Vec::new();
        if step.meta.optional { flags.push("optional".to_string()); }
        if step.meta.fallible { flags.push("fallible".to_string()); }
        if !step.meta.silent.is_empty() {
            let s: Vec<_> = step.meta.silent.iter().map(|s| format!("{s:?}").to_lowercase()).collect();
            flags.push(format!("silent: {}", s.join(", ")));
        }
        if let Some(r) = step.meta.max_retries { flags.push(format!("max-retries: {r}")); }
        if let Some(d) = step.meta.retry_delay { flags.push(format!("retry-delay: {d}s")); }
        for f in &flags { header.push_str(&format!(" [{f}]")); }
        println!("{header}");

        // Action details
        let ai = format!("{indent}    ");
        match &step.action {
            Action::Shell { commands, dir, env, on_return } => {
                for cmd in commands { println!("{ai}sh -c {cmd:?}"); }
                if let Some(d) = dir { println!("{ai}dir: {d}"); }
                if let Some(e) = env {
                    for (k, v) in e { println!("{ai}env: {k}={v}"); }
                }
                if let Some(map) = on_return {
                    println!("{ai}on-return:");
                    for (code, sr) in map {
                        let target = step_ref_label(sr);
                        println!("{ai}  {code} → {target}");
                    }
                }
            }
            Action::Git { repo, dest, if_exists } => {
                println!("{ai}git clone {repo} → {dest}");
                println!("{ai}if-exists: {if_exists:?}");
            }
            Action::Fs { action, recurse, target, if_exists, if_not_exists } => {
                match (action, target) {
                    (FsAction::Create, FsTarget::Path { path }) => {
                        for p in path_list(path) {
                            let kind = if p.ends_with('/') { "dir" } else { "file" };
                            println!("{ai}create {kind}: {p}");
                        }
                    }
                    (FsAction::Delete, FsTarget::Path { path }) => {
                        for p in path_list(path) { println!("{ai}delete: {p}"); }
                    }
                    (act, FsTarget::FromTo { from, to }) => {
                        println!("{ai}{act:?} {from} → {to}");
                    }
                    _ => println!("{ai}{action:?}"),
                }
                if *recurse { println!("{ai}recurse: true"); }
                if let Some(c) = if_exists { println!("{ai}if-exists: {}", condition_label(c)); }
                if let Some(c) = if_not_exists { println!("{ai}if-not-exists: {}", condition_label(c)); }
            }
        }

        // Children
        if !step.children.is_empty() {
            println!("{ai}children:");
            for child in &step.children {
                match child {
                    ChildRef::Id(id) => println!("{ai}  → {id}"),
                    ChildRef::Inline(s) => self.audit_step(s, depth + 1),
                }
            }
        }
        println!();
    }
}

enum CondResult {
    Skip,
    Proceed,
    Panic,
}

fn path_list(spec: &PathSpec) -> Vec<String> {
    match spec {
        PathSpec::Single(s) => vec![s.clone()],
        PathSpec::Multiple(v) => v.clone(),
    }
}

fn step_ref_label(sr: &StepRef) -> String {
    match sr {
        StepRef::Id(id) => id.clone(),
        StepRef::Inline(s) => format!("[inline: {}]", s.name),
    }
}

fn condition_label(c: &Condition) -> String {
    match c {
        Condition::Action(a) => format!("{a:?}").to_lowercase(),
        Condition::Execute { execute } => format!("execute({})", step_ref_label(execute)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn runner(index: HashMap<String, Step>) -> Runner {
        Runner::new(index, false, false, None)
    }

    fn dry_runner() -> Runner {
        Runner::new(HashMap::new(), true, false, None)
    }

    fn shell_step(commands: Vec<&str>, dir: Option<&str>) -> Step {
        Step {
            id: None, name: "test".into(), description: None,
            action: Action::Shell {
                commands: commands.into_iter().map(String::from).collect(),
                dir: dir.map(String::from),
                env: None,
                on_return: None,
            },
            children: vec![], meta: Meta::default(),
        }
    }

    #[test]
    fn shell_runs_command() {
        let dir = tempfile::tempdir().unwrap();
        let step = shell_step(vec!["echo hi > out.txt"], Some(dir.path().to_str().unwrap()));
        runner(HashMap::new()).run_step(&step, 0).unwrap();
        assert!(dir.path().join("out.txt").exists());
    }

    #[test]
    fn shell_dry_run() {
        let dir = tempfile::tempdir().unwrap();
        let step = shell_step(vec!["echo hi > out.txt"], Some(dir.path().to_str().unwrap()));
        dry_runner().run_step(&step, 0).unwrap();
        assert!(!dir.path().join("out.txt").exists());
    }

    #[test]
    fn shell_failure() {
        let step = shell_step(vec!["false"], None);
        assert!(runner(HashMap::new()).run_step(&step, 0).is_err());
    }

    #[test]
    fn shell_fallible() {
        let mut step = shell_step(vec!["false"], None);
        step.meta.fallible = true;
        runner(HashMap::new()).run_step(&step, 0).unwrap();
    }

    #[test]
    fn fs_create_dir() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("newdir");
        let path_str = format!("{}/", target.display());
        let step = Step {
            id: None, name: "create".into(), description: None,
            action: Action::Fs {
                action: FsAction::Create, recurse: false,
                target: FsTarget::Path { path: PathSpec::Single(path_str) },
                if_exists: None, if_not_exists: None,
            },
            children: vec![], meta: Meta::default(),
        };
        runner(HashMap::new()).run_step(&step, 0).unwrap();
        assert!(target.is_dir());
    }

    #[test]
    fn fs_create_file() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("newfile.txt");
        let step = Step {
            id: None, name: "create".into(), description: None,
            action: Action::Fs {
                action: FsAction::Create, recurse: false,
                target: FsTarget::Path { path: PathSpec::Single(target.to_string_lossy().into()) },
                if_exists: None, if_not_exists: None,
            },
            children: vec![], meta: Meta::default(),
        };
        runner(HashMap::new()).run_step(&step, 0).unwrap();
        assert!(target.is_file());
    }

    #[test]
    fn fs_symlink_creates() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src.txt");
        let dst = dir.path().join("dst.txt");
        fs::write(&src, "hello").unwrap();
        let step = Step {
            id: None, name: "link".into(), description: None,
            action: Action::Fs {
                action: FsAction::Symlink, recurse: false,
                target: FsTarget::FromTo { from: src.to_string_lossy().into(), to: dst.to_string_lossy().into() },
                if_exists: None, if_not_exists: None,
            },
            children: vec![], meta: Meta::default(),
        };
        runner(HashMap::new()).run_step(&step, 0).unwrap();
        assert!(dst.is_symlink());
        assert_eq!(fs::read_to_string(&dst).unwrap(), "hello");
    }

    #[test]
    fn fs_copy_file() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src.txt");
        let dst = dir.path().join("dst.txt");
        fs::write(&src, "data").unwrap();
        let step = Step {
            id: None, name: "copy".into(), description: None,
            action: Action::Fs {
                action: FsAction::Copy, recurse: false,
                target: FsTarget::FromTo { from: src.to_string_lossy().into(), to: dst.to_string_lossy().into() },
                if_exists: None, if_not_exists: Some(Condition::Action(ConditionAction::Panic)),
            },
            children: vec![], meta: Meta::default(),
        };
        runner(HashMap::new()).run_step(&step, 0).unwrap();
        assert_eq!(fs::read_to_string(&dst).unwrap(), "data");
    }

    #[test]
    fn fs_move_file() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src.txt");
        let dst = dir.path().join("dst.txt");
        fs::write(&src, "data").unwrap();
        let step = Step {
            id: None, name: "mv".into(), description: None,
            action: Action::Fs {
                action: FsAction::Move, recurse: false,
                target: FsTarget::FromTo { from: src.to_string_lossy().into(), to: dst.to_string_lossy().into() },
                if_exists: None, if_not_exists: None,
            },
            children: vec![], meta: Meta::default(),
        };
        runner(HashMap::new()).run_step(&step, 0).unwrap();
        assert!(!src.exists());
        assert_eq!(fs::read_to_string(&dst).unwrap(), "data");
    }

    #[test]
    fn fs_delete_file() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("del.txt");
        fs::write(&f, "x").unwrap();
        let step = Step {
            id: None, name: "del".into(), description: None,
            action: Action::Fs {
                action: FsAction::Delete, recurse: false,
                target: FsTarget::Path { path: PathSpec::Single(f.to_string_lossy().into()) },
                if_exists: None, if_not_exists: None,
            },
            children: vec![], meta: Meta::default(),
        };
        runner(HashMap::new()).run_step(&step, 0).unwrap();
        assert!(!f.exists());
    }

    #[test]
    fn fs_delete_not_exists_skip() {
        let step = Step {
            id: None, name: "del".into(), description: None,
            action: Action::Fs {
                action: FsAction::Delete, recurse: false,
                target: FsTarget::Path { path: PathSpec::Single("/tmp/nonexistent_devsetup_test".into()) },
                if_exists: None, if_not_exists: Some(Condition::Action(ConditionAction::Skip)),
            },
            children: vec![], meta: Meta::default(),
        };
        runner(HashMap::new()).run_step(&step, 0).unwrap();
    }

    #[test]
    fn children_run_after_parent() {
        let dir = tempfile::tempdir().unwrap();
        let step = Step {
            id: None, name: "parent".into(), description: None,
            action: Action::Shell {
                commands: vec!["echo parent".into()],
                dir: None, env: None, on_return: None,
            },
            children: vec![
                ChildRef::Inline(Box::new(shell_step(vec!["echo child > child.txt"], Some(dir.path().to_str().unwrap())))),
            ],
            meta: Meta::default(),
        };
        runner(HashMap::new()).run_step(&step, 0).unwrap();
        assert!(dir.path().join("child.txt").exists());
    }

    #[test]
    fn children_skip_on_fallible_failure() {
        let dir = tempfile::tempdir().unwrap();
        let step = Step {
            id: None, name: "parent".into(), description: None,
            action: Action::Shell {
                commands: vec!["false".into()],
                dir: None, env: None, on_return: None,
            },
            children: vec![
                ChildRef::Inline(Box::new(shell_step(vec!["echo child > child.txt"], Some(dir.path().to_str().unwrap())))),
            ],
            meta: Meta { optional: false, fallible: true, silent: vec![], ..Meta::default() },
        };
        runner(HashMap::new()).run_step(&step, 0).unwrap();
        assert!(!dir.path().join("child.txt").exists()); // children didn't run
    }

    #[test]
    fn optional_steps_skipped() {
        let steps = vec![Step {
            id: None, name: "opt".into(), description: None,
            action: Action::Shell { commands: vec!["false".into()], dir: None, env: None, on_return: None },
            children: vec![], meta: Meta { optional: true, fallible: false, silent: vec![], ..Meta::default() },
        }];
        runner(HashMap::new()).run_steps(&steps).unwrap(); // would fail if not skipped
    }

    #[test]
    fn on_return_triggers_ref() {
        let dir = tempfile::tempdir().unwrap();
        let handler = Step {
            id: Some("handler".into()), name: "handler".into(), description: None,
            action: Action::Shell {
                commands: vec![format!("echo handled > {}/handled.txt", dir.path().display())],
                dir: None, env: None, on_return: None,
            },
            children: vec![], meta: Meta { optional: true, ..Meta::default() },
        };
        let mut index = HashMap::new();
        index.insert("handler".into(), handler);

        let step = Step {
            id: None, name: "test".into(), description: None,
            action: Action::Shell {
                commands: vec!["exit 42".into()],
                dir: None, env: None,
                on_return: Some(HashMap::from([("_".into(), StepRef::Id("handler".into()))])),
            },
            children: vec![], meta: Meta::default(),
        };
        runner(index).run_step(&step, 0).unwrap();
        assert!(dir.path().join("handled.txt").exists());
    }

    #[test]
    fn on_return_code0_children_still_run() {
        let dir = tempfile::tempdir().unwrap();
        let handler = Step {
            id: Some("handler".into()), name: "handler".into(), description: None,
            action: Action::Shell {
                commands: vec![format!("echo handled > {}/handled.txt", dir.path().display())],
                dir: None, env: None, on_return: None,
            },
            children: vec![], meta: Meta { optional: true, ..Meta::default() },
        };
        let mut index = HashMap::new();
        index.insert("handler".into(), handler);

        let step = Step {
            id: None, name: "test".into(), description: None,
            action: Action::Shell {
                commands: vec!["true".into()],
                dir: None, env: None,
                on_return: Some(HashMap::from([("0".into(), StepRef::Id("handler".into()))])),
            },
            children: vec![
                ChildRef::Inline(Box::new(shell_step(vec!["echo child > child.txt"], Some(dir.path().to_str().unwrap())))),
            ],
            meta: Meta::default(),
        };
        runner(index).run_step(&step, 0).unwrap();
        assert!(dir.path().join("handled.txt").exists());
        assert!(dir.path().join("child.txt").exists()); // children ran too
    }

    #[test]
    fn max_retries_enforced_no_config() {
        // No global max_retries, no per-step max_retries → second entry fails
        let step_a = Step {
            id: Some("a".into()), name: "a".into(), description: None,
            action: Action::Shell { commands: vec!["true".into()], dir: None, env: None, on_return: None },
            children: vec![ChildRef::Id("a".into())],
            meta: Meta::default(),
        };
        let mut index = HashMap::new();
        index.insert("a".into(), step_a.clone());
        let r = Runner::new(index, false, false, None);
        assert!(r.run_step(&step_a, 0).is_err());
    }

    #[test]
    fn max_retries_per_step_allows_retry() {
        let dir = tempfile::tempdir().unwrap();
        let counter_file = dir.path().join("count");
        // Step that writes to a file each time, with max_retries=2
        // We simulate: step runs, child refs back to step, runs again (retry 1), child refs again (retry 2), then child refs again → error
        let step = Step {
            id: Some("counted".into()), name: "counted".into(), description: None,
            action: Action::Shell {
                commands: vec![format!("echo x >> {}", counter_file.display())],
                dir: None, env: None, on_return: None,
            },
            children: vec![ChildRef::Id("counted".into())],
            meta: Meta { max_retries: Some(2), ..Meta::default() },
        };
        let mut index = HashMap::new();
        index.insert("counted".into(), step.clone());
        let r = Runner::new(index, false, false, None);
        // Should run 3 times (1 + 2 retries) then fail on 4th
        let result = r.run_step(&step, 0);
        assert!(result.is_err());
        let content = std::fs::read_to_string(&counter_file).unwrap();
        assert_eq!(content.lines().count(), 3); // ran 3 times
    }

    #[test]
    fn global_max_retries_applies() {
        let step = Step {
            id: Some("a".into()), name: "a".into(), description: None,
            action: Action::Shell { commands: vec!["true".into()], dir: None, env: None, on_return: None },
            children: vec![ChildRef::Id("a".into())],
            meta: Meta::default(), // no per-step override
        };
        let mut index = HashMap::new();
        index.insert("a".into(), step.clone());
        let r = Runner::new(index, false, false, Some(1)); // global allows 1 retry
        let result = r.run_step(&step, 0);
        // Should succeed twice (1 + 1 retry), fail on third
        assert!(result.is_err());
    }
}
