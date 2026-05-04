use crate::config::*;
use crate::path::expand_tilde;
use crate::style;
use std::collections::HashMap;
use std::fmt;
use std::process::Command;

const MAX_ENTRIES: u32 = 64;

#[derive(Debug)]
pub enum ExecError {
    Command(String),
    Io(std::io::Error),
    StepNotFound(String),
    CycleDetected(String),
}

impl fmt::Display for ExecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Command(msg) => write!(f, "{msg}"),
            Self::Io(e) => write!(f, "{e}"),
            Self::StepNotFound(id) => write!(f, "step not found: {id}"),
            Self::CycleDetected(name) => write!(f, "cycle detected (>{MAX_ENTRIES} entries): {name}"),
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
    pub global_retries: Option<u32>,
    entry_counts: RefCell<HashMap<String, u32>>,
}

impl Runner {
    pub fn new(index: HashMap<String, Step>, dry_run: bool, verbose: bool, global_retries: Option<u32>) -> Self {
        Self { index, dry_run, verbose, global_retries, entry_counts: RefCell::new(HashMap::new()) }
    }

    pub fn run_steps(&self, steps: &[Step]) -> Result<(), ExecError> {
        if !self.dry_run && Self::needs_sudo(steps) {
            self.preflight_sudo()?;
        }
        for step in steps {
            if step.meta.optional { continue; }
            self.run_step(step, 0)?;
        }
        Ok(())
    }

    fn needs_sudo(steps: &[Step]) -> bool {
        steps.iter().any(|s| s.meta.sudo)
    }

    fn preflight_sudo(&self) -> Result<(), ExecError> {
        println!("{}", style::render("<fy>🔒 sudo required — validating credentials...</f>"));
        let status = Command::new("sudo").arg("-v").status()?;
        if !status.success() {
            return Err(ExecError::Command("sudo authentication failed".into()));
        }
        Ok(())
    }

    pub fn run_step(&self, step: &Step, depth: usize) -> Result<(), ExecError> {
        let indent = "  ".repeat(depth);

        // Hard cycle limit
        if let Some(id) = &step.id {
            let mut counts = self.entry_counts.borrow_mut();
            let count = counts.entry(id.clone()).or_insert(0);
            *count += 1;
            if *count > MAX_ENTRIES { return Err(ExecError::CycleDetected(step.name.clone())); }
        }

        println!("{indent}{}", style::render(&format!("<fg>→</f> <mb>{}</m>", step.name)));

        let max_retries = step.meta.retries.or(self.global_retries).unwrap_or(0);
        let mut last_err = None;

        for attempt in 0..=max_retries {
            if attempt > 0
                && let Some(delay) = step.meta.retry_delay
            {
                if !self.dry_run {
                    println!("{indent}  {}", style::render(&format!("<fy>⏳ retrying in {delay}s...</f>")));
                    std::thread::sleep(std::time::Duration::from_secs_f64(delay));
                } else {
                    println!("{indent}  {}", style::render(&format!("<md>[dry-run]</m> would sleep {delay}s before retry")));
                }
            }

            match self.exec_action(&step.action, &step.meta, &indent, depth) {
                Ok(code) => {
                    // Resolve handler: on-return[code] → on-return["_"] → on-success
                    let handler = self.resolve_handler(step, code, true);
                    if let Some(refs) = handler {
                        self.run_step_refs(refs, depth + 1)?;
                    }
                    // Run then steps
                    for child in &step.then { self.run_child(child, depth + 1)?; }
                    return Ok(());
                }
                Err(e) => { last_err = Some(e); }
            }
        }

        // All retries exhausted — resolve failure handler
        let err = last_err.unwrap();
        let handler = self.resolve_handler(step, -1, false);
        if let Some(refs) = handler {
            self.run_step_refs(refs, depth + 1)?;
            // Handler caught it — run then steps
            for child in &step.then { self.run_child(child, depth + 1)?; }
            return Ok(());
        }

        if step.meta.fallible {
            println!("{indent}  {}", style::render(&format!("<fy>⚠ failed (fallible):</f> {err}")));
            return Ok(()); // don't run then
        }
        Err(err)
    }

    /// Resolve which handler to run. Returns None if no handler matches.
    fn resolve_handler<'a>(&self, step: &'a Step, code: i32, success: bool) -> Option<&'a StepRefs> {
        // Check on-return for exact code
        if let Some(map) = &step.on_return {
            let key = code.to_string();
            if let Some(refs) = map.get(&key) { return Some(refs); }
            if let Some(refs) = map.get("_") { return Some(refs); }
        }
        // Fall back to on-success / on-failure
        if success { step.on_success.as_ref() } else { step.on_failure.as_ref() }
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

    fn run_step_refs(&self, refs: &StepRefs, depth: usize) -> Result<(), ExecError> {
        match refs {
            StepRefs::Single(sr) => self.run_step_ref(sr, depth),
            StepRefs::Multiple(v) => { for sr in v { self.run_step_ref(sr, depth)?; } Ok(()) }
        }
    }

    /// Execute an action, returning the exit code (0 for non-shell actions on success).
    fn exec_action(&self, action: &Action, meta: &Meta, indent: &str, _depth: usize) -> Result<i32, ExecError> {
        match action {
            Action::Shell { commands, dir, env } => self.exec_shell(commands, dir.as_deref(), env.as_ref(), meta, indent, meta.sudo),
            Action::Git { repo, dest, on_conflict } => { self.exec_git(repo, dest, on_conflict, meta, indent)?; Ok(0) }
            Action::Fs { op, if_exists, if_not_exists } => { self.exec_fs(op, if_exists.as_ref(), if_not_exists.as_ref(), indent)?; Ok(0) }
        }
    }

    fn maybe_print(&self, stdout: &[u8], stderr: &[u8], meta: &Meta) {
        let show_out = !meta.silent.contains(&Silent::Stdout) || self.verbose;
        let show_err = !meta.silent.contains(&Silent::Stderr) || self.verbose;
        if show_out && !stdout.is_empty() { print!("{}", String::from_utf8_lossy(stdout)); }
        if show_err && !stderr.is_empty() { eprint!("{}", String::from_utf8_lossy(stderr)); }
    }

    // ── Shell ──

    fn exec_shell(
        &self,
        commands: &[String],
        dir: Option<&str>,
        env: Option<&HashMap<String, String>>,
        meta: &Meta,
        indent: &str,
        sudo: bool,
    ) -> Result<i32, ExecError> {
        let mut last_code = 0;
        for cmd in commands {
            if self.dry_run {
                let prefix = if sudo { "sudo sh -c" } else { "sh -c" };
                println!("{indent}  [dry-run] {prefix} {cmd:?}");
                if let Some(d) = dir { println!("{indent}    dir: {d}"); }
                if let Some(e) = env { println!("{indent}    env: {e:?}"); }
                continue;
            }
            let mut proc = if sudo {
                let mut p = Command::new("sudo");
                p.arg("sh").arg("-c").arg(cmd);
                p
            } else {
                let mut p = Command::new("sh");
                p.arg("-c").arg(cmd);
                p
            };
            if let Some(d) = dir { proc.current_dir(expand_tilde(d)); }
            if let Some(e) = env { proc.envs(e); }
            let output = proc.output()?;
            last_code = output.status.code().unwrap_or(-1);
            self.maybe_print(&output.stdout, &output.stderr, meta);
            if !output.status.success() {
                return Err(ExecError::Command(format!("command failed (exit {last_code}): {cmd}")));
            }
        }
        Ok(last_code)
    }

    // ── Git ──

    fn exec_git(&self, repo: &str, dest: &str, on_conflict: &GitOnConflict, meta: &Meta, indent: &str) -> Result<(), ExecError> {
        let dest_path = expand_tilde(dest);
        let exists = dest_path.exists();

        if self.dry_run {
            if exists {
                println!("{indent}  [dry-run] dest {} exists → {on_conflict:?}", dest_path.display());
            } else {
                println!("{indent}  [dry-run] git clone {repo} {}", dest_path.display());
            }
            return Ok(());
        }

        if !exists {
            let output = Command::new("git").args(["clone", repo, &dest_path.to_string_lossy()]).output()?;
            self.maybe_print(&output.stdout, &output.stderr, meta);
            if !output.status.success() {
                return Err(ExecError::Command(format!("git clone failed ({})", output.status)));
            }
            return Ok(());
        }

        match on_conflict {
            GitOnConflict::Skip => println!("{indent}  {}", style::render(&format!("<fy>skipped (exists):</f> {}", dest_path.display()))),
            GitOnConflict::Pull => {
                let output = Command::new("git").args(["-C", &dest_path.to_string_lossy(), "pull"]).output()?;
                self.maybe_print(&output.stdout, &output.stderr, meta);
                if !output.status.success() {
                    return Err(ExecError::Command(format!("git pull failed ({})", output.status)));
                }
            }
            GitOnConflict::Fail => {
                return Err(ExecError::Command(format!("dest already exists: {}", dest_path.display())));
            }
        }
        Ok(())
    }

    // ── FS ──

    fn exec_fs(&self, op: &FsOp, if_exists: Option<&Condition>, if_not_exists: Option<&Condition>, indent: &str) -> Result<(), ExecError> {
        match op {
            FsOp::Create { path, recurse, content } => self.fs_create(path, *recurse, content.as_deref(), if_exists, indent),
            FsOp::Symlink { from, to } => self.fs_symlink(from, to, if_exists, indent),
            FsOp::Copy { from, to } => self.fs_copy(from, to, if_exists, if_not_exists, indent),
            FsOp::Move { from, to } => self.fs_move(from, to, if_exists, if_not_exists, indent),
            FsOp::Delete { path, recurse } => self.fs_delete(path, *recurse, if_not_exists, indent),
        }
    }

    fn fs_create(&self, path: &PathSpec, recurse: bool, content: Option<&str>, if_exists: Option<&Condition>, indent: &str) -> Result<(), ExecError> {
        for p in path_list(path) {
            let full = expand_tilde(&p);
            let is_dir = p.ends_with('/');
            if self.dry_run {
                let kind = if is_dir { "dir" } else { "file" };
                println!("{indent}  [dry-run] create {kind}: {}", full.display());
                if let Some(c) = content { println!("{indent}    content: {c:?}"); }
                continue;
            }
            if full.exists() {
                match self.handle_condition(if_exists, indent)? {
                    CondResult::Skip => continue,
                    CondResult::Proceed => {}
                    CondResult::Panic => return Err(ExecError::Command(format!("already exists: {}", full.display()))),
                }
            }
            if is_dir {
                if recurse { std::fs::create_dir_all(&full)?; } else { std::fs::create_dir(&full)?; }
            } else {
                if recurse && let Some(parent) = full.parent() { std::fs::create_dir_all(parent)?; }
                if let Some(c) = content {
                    std::fs::write(&full, c)?;
                } else {
                    std::fs::File::create(&full)?;
                }
            }
            println!("{indent}  {}", style::render(&format!("<fg>created:</f> {}", full.display())));
        }
        Ok(())
    }

    fn fs_symlink(&self, from: &str, to: &str, if_exists: Option<&Condition>, indent: &str) -> Result<(), ExecError> {
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
        println!("{indent}  {}", style::render(&format!("<fg>symlinked</f> {} → {}", src.display(), dst.display())));
        Ok(())
    }

    fn fs_copy(&self, from: &str, to: &str, if_exists: Option<&Condition>, if_not_exists: Option<&Condition>, indent: &str) -> Result<(), ExecError> {
        let src = expand_tilde(from);
        let dst = expand_tilde(to);
        if self.dry_run {
            println!("{indent}  [dry-run] copy {} → {}", src.display(), dst.display());
            return Ok(());
        }
        if !src.exists() { return self.handle_not_exists(if_not_exists, &src, indent); }
        if dst.exists() {
            match self.handle_condition(if_exists, indent)? {
                CondResult::Skip => return Ok(()),
                CondResult::Proceed => {}
                CondResult::Panic => return Err(ExecError::Command(format!("already exists: {}", dst.display()))),
            }
        }
        std::fs::copy(&src, &dst)?;
        println!("{indent}  {}", style::render(&format!("<fg>copied</f> {} → {}", src.display(), dst.display())));
        Ok(())
    }

    fn fs_move(&self, from: &str, to: &str, if_exists: Option<&Condition>, if_not_exists: Option<&Condition>, indent: &str) -> Result<(), ExecError> {
        let src = expand_tilde(from);
        let dst = expand_tilde(to);
        if self.dry_run {
            println!("{indent}  [dry-run] move {} → {}", src.display(), dst.display());
            return Ok(());
        }
        if !src.exists() { return self.handle_not_exists(if_not_exists, &src, indent); }
        if dst.exists() {
            match self.handle_condition(if_exists, indent)? {
                CondResult::Skip => return Ok(()),
                CondResult::Proceed => { std::fs::remove_file(&dst).or_else(|_| std::fs::remove_dir_all(&dst))?; }
                CondResult::Panic => return Err(ExecError::Command(format!("already exists: {}", dst.display()))),
            }
        }
        std::fs::rename(&src, &dst)?;
        println!("{indent}  {}", style::render(&format!("<fg>moved</f> {} → {}", src.display(), dst.display())));
        Ok(())
    }

    fn fs_delete(&self, path: &PathSpec, recurse: bool, if_not_exists: Option<&Condition>, indent: &str) -> Result<(), ExecError> {
        for p in path_list(path) {
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
            println!("{indent}  {}", style::render(&format!("<fg>deleted:</f> {}", full.display())));
        }
        Ok(())
    }

    // ── Condition helpers ──

    fn handle_condition(&self, cond: Option<&Condition>, _indent: &str) -> Result<CondResult, ExecError> {
        match cond {
            None | Some(Condition::Action(ConditionAction::Panic)) => Ok(CondResult::Panic),
            Some(Condition::Action(ConditionAction::Skip)) => Ok(CondResult::Skip),
            Some(Condition::Action(ConditionAction::Overwrite)) => Ok(CondResult::Proceed),
            Some(Condition::Action(ConditionAction::Append)) => Ok(CondResult::Proceed),
            Some(Condition::Execute { execute }) => {
                self.run_step_ref(execute, 0)?;
                Ok(CondResult::Skip)
            }
        }
    }

    fn handle_not_exists(&self, cond: Option<&Condition>, path: &std::path::Path, _indent: &str) -> Result<(), ExecError> {
        match cond {
            None | Some(Condition::Action(ConditionAction::Panic)) => Err(ExecError::Command(format!("does not exist: {}", path.display()))),
            Some(Condition::Action(ConditionAction::Skip)) => Ok(()),
            Some(Condition::Execute { execute }) => self.run_step_ref(execute, 0),
            _ => Err(ExecError::Command(format!("does not exist: {}", path.display()))),
        }
    }

    // ── Dry-run audit ──

    pub fn dry_run_audit(&self, config: &Config, steps: &[Step]) {
        // Top-level config info
        println!("{}", style::render(&format!("<mb>version:</m> {}", config.version)));
        if self.verbose
            && let Some(desc) = &config.description
        {
            println!("{}", style::render(&format!("<md>{desc}</m>")));
        }
        if let Some(r) = config.retries { println!("{}", style::render(&format!("<mb>retries:</m> {r}"))); }

        let optional_count = steps.iter().filter(|s| s.meta.optional).count();
        let fallible_count = steps.iter().filter(|s| s.meta.fallible).count();
        let sudo_count = steps.iter().filter(|s| s.meta.sudo).count();
        let mut counts = format!("{} steps, {} optional, {} fallible", steps.len(), optional_count, fallible_count);
        if sudo_count > 0 { counts.push_str(&format!(", {} sudo", sudo_count)); }
        println!("{}", style::render(&format!("<md>({counts})</m>\n")));

        for step in steps { self.audit_step(step, 0); }
        println!("{}", style::render(&format!("<mb>Summary:</m> {counts}")));
    }

    fn audit_step(&self, step: &Step, depth: usize) {
        let indent = "  ".repeat(depth);
        let mut header = format!("{indent}<fg>→</f> <mb>{}</m>", step.name);
        if let Some(id) = &step.id { header.push_str(&format!(" <md>(id: <fc>{id}</f>)</m>")); }
        let mut flags = Vec::new();
        if step.meta.optional { flags.push("<fy>optional</f>".to_string()); }
        if step.meta.fallible { flags.push("<fy>fallible</f>".to_string()); }
        if step.meta.sudo { flags.push("<fy>sudo</f>".to_string()); }
        if !step.meta.silent.is_empty() {
            let s: Vec<_> = step.meta.silent.iter().map(|s| format!("{s:?}").to_lowercase()).collect();
            flags.push(format!("<fy>silent: {}</f>", s.join(", ")));
        }
        if let Some(r) = step.meta.retries { flags.push(format!("<fy>retries: {r}</f>")); }
        if let Some(d) = step.meta.retry_delay { flags.push(format!("<fy>retry-delay: {d}s</f>")); }
        for f in &flags { header.push_str(&format!(" [{f}]")); }
        println!("{}", style::render(&header));
        if self.verbose
            && let Some(desc) = &step.description
        {
            println!("{indent}  {}", style::render(&format!("<md>{desc}</m>")));
        }

        let ai = format!("{indent}    ");
        match &step.action {
            Action::Shell { commands, dir, env } => {
                let prefix = if step.meta.sudo { "sudo sh -c" } else { "sh -c" };
                for cmd in commands { println!("{ai}{}", style::render(&format!("<md>{prefix}</m> {cmd:?}"))); }
                if let Some(d) = dir { println!("{ai}{}", style::render(&format!("<md>dir:</m> {d}"))); }
                if let Some(e) = env {
                    for (k, v) in e { println!("{ai}{}", style::render(&format!("<md>env:</m> {k}={v}"))); }
                }
            }
            Action::Git { repo, dest, on_conflict } => {
                println!("{ai}{}", style::render(&format!("<md>git clone</m> {repo} → {dest}")));
                println!("{ai}{}", style::render(&format!("<md>on-conflict:</m> {on_conflict:?}")));
            }
            Action::Fs { op, if_exists, if_not_exists } => {
                match op {
                    FsOp::Create { path, recurse, content } => {
                        for p in path_list(path) {
                            let kind = if p.ends_with('/') { "dir" } else { "file" };
                            println!("{ai}{}", style::render(&format!("<md>create {kind}:</m> {p}")));
                        }
                        if *recurse { println!("{ai}{}", style::render("<md>recurse:</m> true")); }
                        if let Some(c) = content { println!("{ai}{}", style::render(&format!("<md>content:</m> {c:?}"))); }
                    }
                    FsOp::Symlink { from, to } => println!("{ai}{}", style::render(&format!("<md>symlink</m> {from} → {to}"))),
                    FsOp::Copy { from, to } => println!("{ai}{}", style::render(&format!("<md>copy</m> {from} → {to}"))),
                    FsOp::Move { from, to } => println!("{ai}{}", style::render(&format!("<md>move</m> {from} → {to}"))),
                    FsOp::Delete { path, recurse } => {
                        for p in path_list(path) { println!("{ai}{}", style::render(&format!("<md>delete:</m> {p}"))); }
                        if *recurse { println!("{ai}{}", style::render("<md>recurse:</m> true")); }
                    }
                }
                if let Some(c) = if_exists { println!("{ai}{}", style::render(&format!("<md>if-exists:</m> {}", condition_label(c)))); }
                if let Some(c) = if_not_exists { println!("{ai}{}", style::render(&format!("<md>if-not-exists:</m> {}", condition_label(c)))); }
            }
        }

        // on-success / on-failure / on-return
        if let Some(refs) = &step.on_success { println!("{ai}{}", style::render(&format!("<md>on-success:</m> {}", step_refs_label(refs)))); }
        if let Some(refs) = &step.on_failure { println!("{ai}{}", style::render(&format!("<md>on-failure:</m> {}", step_refs_label(refs)))); }
        if let Some(map) = &step.on_return {
            println!("{ai}{}", style::render("<md>on-return:</m>"));
            for (code, refs) in map { println!("{ai}  {}", style::render(&format!("{code} → <fc>{}</f>", step_refs_label(refs)))); }
        }

        // then
        if !step.then.is_empty() {
            println!("{ai}{}", style::render("<md>then:</m>"));
            for child in &step.then {
                match child {
                    ChildRef::Id(id) => println!("{ai}  {}", style::render(&format!("→ <fc>{id}</f>"))),
                    ChildRef::Inline(s) => self.audit_step(s, depth + 1),
                }
            }
        }
        println!();
    }
}

// ── Helpers ──

enum CondResult { Skip, Proceed, Panic }

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

fn step_refs_label(refs: &StepRefs) -> String {
    match refs {
        StepRefs::Single(sr) => step_ref_label(sr),
        StepRefs::Multiple(v) => v.iter().map(step_ref_label).collect::<Vec<_>>().join(", "),
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
                dir: dir.map(String::from), env: None,
            },
            on_success: None, on_failure: None, on_return: None,
            then: vec![], meta: Meta::default(),
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
                op: FsOp::Create { path: PathSpec::Single(path_str), recurse: false, content: None },
                if_exists: None, if_not_exists: None,
            },
            on_success: None, on_failure: None, on_return: None,
            then: vec![], meta: Meta::default(),
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
                op: FsOp::Create { path: PathSpec::Single(target.to_string_lossy().into()), recurse: false, content: None },
                if_exists: None, if_not_exists: None,
            },
            on_success: None, on_failure: None, on_return: None,
            then: vec![], meta: Meta::default(),
        };
        runner(HashMap::new()).run_step(&step, 0).unwrap();
        assert!(target.is_file());
    }

    #[test]
    fn fs_create_file_with_content() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("hello.txt");
        let step = Step {
            id: None, name: "create".into(), description: None,
            action: Action::Fs {
                op: FsOp::Create { path: PathSpec::Single(target.to_string_lossy().into()), recurse: false, content: Some("hello world".into()) },
                if_exists: None, if_not_exists: None,
            },
            on_success: None, on_failure: None, on_return: None,
            then: vec![], meta: Meta::default(),
        };
        runner(HashMap::new()).run_step(&step, 0).unwrap();
        assert_eq!(fs::read_to_string(&target).unwrap(), "hello world");
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
                op: FsOp::Symlink { from: src.to_string_lossy().into(), to: dst.to_string_lossy().into() },
                if_exists: None, if_not_exists: None,
            },
            on_success: None, on_failure: None, on_return: None,
            then: vec![], meta: Meta::default(),
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
                op: FsOp::Copy { from: src.to_string_lossy().into(), to: dst.to_string_lossy().into() },
                if_exists: None, if_not_exists: Some(Condition::Action(ConditionAction::Panic)),
            },
            on_success: None, on_failure: None, on_return: None,
            then: vec![], meta: Meta::default(),
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
                op: FsOp::Move { from: src.to_string_lossy().into(), to: dst.to_string_lossy().into() },
                if_exists: None, if_not_exists: None,
            },
            on_success: None, on_failure: None, on_return: None,
            then: vec![], meta: Meta::default(),
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
                op: FsOp::Delete { path: PathSpec::Single(f.to_string_lossy().into()), recurse: false },
                if_exists: None, if_not_exists: None,
            },
            on_success: None, on_failure: None, on_return: None,
            then: vec![], meta: Meta::default(),
        };
        runner(HashMap::new()).run_step(&step, 0).unwrap();
        assert!(!f.exists());
    }

    #[test]
    fn fs_delete_not_exists_skip() {
        let step = Step {
            id: None, name: "del".into(), description: None,
            action: Action::Fs {
                op: FsOp::Delete { path: PathSpec::Single("/tmp/nonexistent_rig_test".into()), recurse: false },
                if_exists: None, if_not_exists: Some(Condition::Action(ConditionAction::Skip)),
            },
            on_success: None, on_failure: None, on_return: None,
            then: vec![], meta: Meta::default(),
        };
        runner(HashMap::new()).run_step(&step, 0).unwrap();
    }

    #[test]
    fn then_runs_after_parent() {
        let dir = tempfile::tempdir().unwrap();
        let step = Step {
            id: None, name: "parent".into(), description: None,
            action: Action::Shell { commands: vec!["echo parent".into()], dir: None, env: None },
            on_success: None, on_failure: None, on_return: None,
            then: vec![
                ChildRef::Inline(Box::new(shell_step(vec!["echo child > child.txt"], Some(dir.path().to_str().unwrap())))),
            ],
            meta: Meta::default(),
        };
        runner(HashMap::new()).run_step(&step, 0).unwrap();
        assert!(dir.path().join("child.txt").exists());
    }

    #[test]
    fn then_skipped_on_fallible_failure() {
        let dir = tempfile::tempdir().unwrap();
        let step = Step {
            id: None, name: "parent".into(), description: None,
            action: Action::Shell { commands: vec!["false".into()], dir: None, env: None },
            on_success: None, on_failure: None, on_return: None,
            then: vec![
                ChildRef::Inline(Box::new(shell_step(vec!["echo child > child.txt"], Some(dir.path().to_str().unwrap())))),
            ],
            meta: Meta { fallible: true, ..Meta::default() },
        };
        runner(HashMap::new()).run_step(&step, 0).unwrap();
        assert!(!dir.path().join("child.txt").exists());
    }

    #[test]
    fn optional_steps_skipped() {
        let steps = vec![Step {
            id: None, name: "opt".into(), description: None,
            action: Action::Shell { commands: vec!["false".into()], dir: None, env: None },
            on_success: None, on_failure: None, on_return: None,
            then: vec![], meta: Meta { optional: true, ..Meta::default() },
        }];
        runner(HashMap::new()).run_steps(&steps).unwrap();
    }

    #[test]
    fn on_success_triggers() {
        let dir = tempfile::tempdir().unwrap();
        let handler = Step {
            id: Some("handler".into()), name: "handler".into(), description: None,
            action: Action::Shell {
                commands: vec![format!("echo handled > {}/handled.txt", dir.path().display())],
                dir: None, env: None,
            },
            on_success: None, on_failure: None, on_return: None,
            then: vec![], meta: Meta { optional: true, ..Meta::default() },
        };
        let mut index = HashMap::new();
        index.insert("handler".into(), handler);

        let step = Step {
            id: None, name: "test".into(), description: None,
            action: Action::Shell { commands: vec!["true".into()], dir: None, env: None },
            on_success: Some(StepRefs::Single(StepRef::Id("handler".into()))),
            on_failure: None, on_return: None,
            then: vec![], meta: Meta::default(),
        };
        runner(index).run_step(&step, 0).unwrap();
        assert!(dir.path().join("handled.txt").exists());
    }

    #[test]
    fn on_failure_triggers() {
        let dir = tempfile::tempdir().unwrap();
        let handler = Step {
            id: Some("handler".into()), name: "handler".into(), description: None,
            action: Action::Shell {
                commands: vec![format!("echo handled > {}/handled.txt", dir.path().display())],
                dir: None, env: None,
            },
            on_success: None, on_failure: None, on_return: None,
            then: vec![], meta: Meta { optional: true, ..Meta::default() },
        };
        let mut index = HashMap::new();
        index.insert("handler".into(), handler);

        let step = Step {
            id: None, name: "test".into(), description: None,
            action: Action::Shell { commands: vec!["false".into()], dir: None, env: None },
            on_success: None,
            on_failure: Some(StepRefs::Single(StepRef::Id("handler".into()))),
            on_return: None,
            then: vec![], meta: Meta::default(),
        };
        runner(index).run_step(&step, 0).unwrap();
        assert!(dir.path().join("handled.txt").exists());
    }

    #[test]
    fn on_return_overrides_on_success() {
        let dir = tempfile::tempdir().unwrap();
        let special = Step {
            id: Some("special".into()), name: "special".into(), description: None,
            action: Action::Shell {
                commands: vec![format!("echo special > {}/special.txt", dir.path().display())],
                dir: None, env: None,
            },
            on_success: None, on_failure: None, on_return: None,
            then: vec![], meta: Meta { optional: true, ..Meta::default() },
        };
        let generic = Step {
            id: Some("generic".into()), name: "generic".into(), description: None,
            action: Action::Shell {
                commands: vec![format!("echo generic > {}/generic.txt", dir.path().display())],
                dir: None, env: None,
            },
            on_success: None, on_failure: None, on_return: None,
            then: vec![], meta: Meta { optional: true, ..Meta::default() },
        };
        let mut index = HashMap::new();
        index.insert("special".into(), special);
        index.insert("generic".into(), generic);

        let step = Step {
            id: None, name: "test".into(), description: None,
            action: Action::Shell { commands: vec!["true".into()], dir: None, env: None },
            on_success: Some(StepRefs::Single(StepRef::Id("generic".into()))),
            on_failure: None,
            on_return: Some(HashMap::from([("0".into(), StepRefs::Single(StepRef::Id("special".into())))])),
            then: vec![], meta: Meta::default(),
        };
        runner(index).run_step(&step, 0).unwrap();
        assert!(dir.path().join("special.txt").exists());
        assert!(!dir.path().join("generic.txt").exists());
    }

    #[test]
    fn on_return_wildcard_overrides_on_failure() {
        let dir = tempfile::tempdir().unwrap();
        let handler = Step {
            id: Some("catch".into()), name: "catch".into(), description: None,
            action: Action::Shell {
                commands: vec![format!("echo caught > {}/caught.txt", dir.path().display())],
                dir: None, env: None,
            },
            on_success: None, on_failure: None, on_return: None,
            then: vec![], meta: Meta { optional: true, ..Meta::default() },
        };
        let mut index = HashMap::new();
        index.insert("catch".into(), handler);

        let step = Step {
            id: None, name: "test".into(), description: None,
            action: Action::Shell { commands: vec!["exit 42".into()], dir: None, env: None },
            on_success: None,
            on_failure: Some(StepRefs::Single(StepRef::Id("catch".into()))),
            on_return: Some(HashMap::from([("_".into(), StepRefs::Single(StepRef::Id("catch".into())))])),
            then: vec![], meta: Meta::default(),
        };
        runner(index).run_step(&step, 0).unwrap();
        assert!(dir.path().join("caught.txt").exists());
    }

    #[test]
    fn on_success_then_both_run() {
        let dir = tempfile::tempdir().unwrap();
        let handler = Step {
            id: Some("handler".into()), name: "handler".into(), description: None,
            action: Action::Shell {
                commands: vec![format!("echo handled > {}/handled.txt", dir.path().display())],
                dir: None, env: None,
            },
            on_success: None, on_failure: None, on_return: None,
            then: vec![], meta: Meta { optional: true, ..Meta::default() },
        };
        let mut index = HashMap::new();
        index.insert("handler".into(), handler);

        let step = Step {
            id: None, name: "test".into(), description: None,
            action: Action::Shell { commands: vec!["true".into()], dir: None, env: None },
            on_success: Some(StepRefs::Single(StepRef::Id("handler".into()))),
            on_failure: None, on_return: None,
            then: vec![
                ChildRef::Inline(Box::new(shell_step(vec!["echo child > child.txt"], Some(dir.path().to_str().unwrap())))),
            ],
            meta: Meta::default(),
        };
        runner(index).run_step(&step, 0).unwrap();
        assert!(dir.path().join("handled.txt").exists());
        assert!(dir.path().join("child.txt").exists());
    }

    #[test]
    fn auto_retry_on_failure() {
        let dir = tempfile::tempdir().unwrap();
        let counter = dir.path().join("count");
        let step = Step {
            id: None, name: "retry-test".into(), description: None,
            action: Action::Shell {
                commands: vec![format!("echo x >> {} && false", counter.display())],
                dir: None, env: None,
            },
            on_success: None, on_failure: None, on_return: None,
            then: vec![], meta: Meta { retries: Some(2), ..Meta::default() },
        };
        let result = runner(HashMap::new()).run_step(&step, 0);
        assert!(result.is_err());
        let content = fs::read_to_string(&counter).unwrap();
        assert_eq!(content.lines().count(), 3); // 1 + 2 retries
    }

    #[test]
    fn cycle_detected() {
        let step = Step {
            id: Some("a".into()), name: "a".into(), description: None,
            action: Action::Shell { commands: vec!["true".into()], dir: None, env: None },
            on_success: None, on_failure: None, on_return: None,
            then: vec![ChildRef::Id("a".into())],
            meta: Meta::default(),
        };
        let mut index = HashMap::new();
        index.insert("a".into(), step.clone());
        let r = Runner::new(index, false, false, None);
        let result = r.run_step(&step, 0);
        assert!(result.is_err());
    }
}
