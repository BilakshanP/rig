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
            Self::CycleDetected(name) => {
                write!(f, "cycle detected (>{MAX_ENTRIES} entries): {name}")
            }
        }
    }
}

impl From<std::io::Error> for ExecError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

use std::sync::Mutex;

pub struct Runner {
    pub index: HashMap<String, Step>,
    pub dry_run: bool,
    pub verbose: bool,
    pub quiet: u8,
    pub cli_silent: bool,
    pub config_meta: Meta,
    pub scope: Mutex<crate::vars::Scope>,
    /// Present when we're running from a `.rig` bundle: carries the staging
    /// root + binary matcher + cleanup policy so `fs.copy` can render file
    /// contents when the source lives inside the bundle.
    pub bundle: Option<crate::bundle::BundleCtx>,
    log_file: Mutex<Option<std::fs::File>>,
    entry_counts: Mutex<HashMap<String, u32>>,
}

impl Runner {
    pub fn new(
        index: HashMap<String, Step>,
        dry_run: bool,
        verbose: bool,
        config_meta: Meta,
        scope: crate::vars::Scope,
    ) -> Self {
        Self::new_inner(index, dry_run, verbose, config_meta, scope, None)
    }

    /// Construct a runner that knows it's executing a `.rig` bundle — passes
    /// through the staging context so `fs.copy` can render templated file
    /// contents.
    pub fn new_with_bundle(
        index: HashMap<String, Step>,
        dry_run: bool,
        verbose: bool,
        config_meta: Meta,
        scope: crate::vars::Scope,
        bundle: crate::bundle::BundleCtx,
    ) -> Self {
        Self::new_inner(index, dry_run, verbose, config_meta, scope, Some(bundle))
    }

    fn new_inner(
        index: HashMap<String, Step>,
        dry_run: bool,
        verbose: bool,
        config_meta: Meta,
        scope: crate::vars::Scope,
        bundle: Option<crate::bundle::BundleCtx>,
    ) -> Self {
        let log_file = if !dry_run {
            config_meta.log.as_ref().and_then(|p| {
                let path = crate::path::expand_tilde(p);
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent).ok();
                }
                std::fs::File::create(&path).ok()
            })
        } else {
            None
        };
        Self {
            index,
            dry_run,
            verbose,
            quiet: 0,
            cli_silent: false,
            config_meta,
            scope: Mutex::new(scope),
            bundle,
            log_file: Mutex::new(log_file),
            entry_counts: Mutex::new(HashMap::new()),
        }
    }

    pub fn run_steps(&self, steps: &[Step]) -> Result<(), ExecError> {
        #[cfg(unix)]
        if !self.dry_run && (self.config_meta.sudo || steps.iter().any(|s| s.meta.sudo)) {
            self.preflight_sudo()?;
        }
        #[cfg(windows)]
        if self.config_meta.sudo || steps.iter().any(|s| s.meta.sudo) {
            println!(
                "{}",
                style::render(
                    "<fy>warning:</f> sudo is not supported on Windows; ignoring sudo flag"
                )
            );
        }
        for step in steps {
            if step.meta.optional {
                continue;
            }
            self.run_step(step, 0)?;
        }
        Ok(())
    }

    /// Run steps in parallel based on depends-on DAG.
    /// Steps whose dependencies are satisfied run concurrently.
    pub fn run_steps_parallel(&self, steps: &[Step]) -> Result<(), ExecError> {
        use std::collections::{HashMap, HashSet, VecDeque};

        // Collect non-optional steps with IDs into the DAG
        let id_steps: HashMap<&str, &Step> = steps
            .iter()
            .filter(|s| !s.meta.optional && s.id.is_some())
            .map(|s| (s.id.as_deref().unwrap(), s))
            .collect();

        // If no DAG structure, fall back to sequential
        let has_dag = steps.iter().any(|s| !s.depends_on.is_empty());
        if !has_dag {
            for step in steps {
                if step.meta.optional {
                    continue;
                }
                self.run_step(step, 0)?;
            }
            return Ok(());
        }

        // Kahn's algorithm: process levels
        let mut in_degree: HashMap<&str, usize> = HashMap::new();
        for step in steps {
            if step.meta.optional || step.id.is_none() {
                continue;
            }
            let id = step.id.as_deref().unwrap();
            let count = step
                .depends_on
                .iter()
                .filter(|d| id_steps.contains_key(d.as_str()))
                .count();
            in_degree.insert(id, count);
        }

        // Build reverse adjacency
        let mut dependents: HashMap<&str, Vec<&str>> = HashMap::new();
        for step in steps {
            if step.meta.optional || step.id.is_none() {
                continue;
            }
            let id = step.id.as_deref().unwrap();
            for dep in &step.depends_on {
                dependents.entry(dep.as_str()).or_default().push(id);
            }
        }

        let mut queue: VecDeque<&str> = in_degree
            .iter()
            .filter(|(_, deg)| **deg == 0)
            .map(|(id, _)| *id)
            .collect();

        let mut completed: HashSet<&str> = HashSet::new();

        // Process level by level — steps in same level run sequentially
        // (true parallelism requires Runner to be Sync; for now we get
        // correct ordering with the DAG and can parallelize later)
        while !queue.is_empty() {
            let level: Vec<&str> = queue.drain(..).collect();

            // Run all steps in this level concurrently
            let errors: Mutex<Vec<String>> = Mutex::new(Vec::new());
            std::thread::scope(|scope| {
                let handles: Vec<_> = level
                    .iter()
                    .filter_map(|&id| id_steps.get(id).map(|step| (id, *step)))
                    .map(|(id, step)| {
                        scope.spawn(move || {
                            let result = self.run_step(step, 0);
                            (id, result)
                        })
                    })
                    .collect();

                for handle in handles {
                    let (id, result) = handle.join().unwrap();
                    completed.insert(id);
                    if let Err(e) = result {
                        if let Some(step) = id_steps.get(id) {
                            if !step.meta.fallible {
                                errors.lock().unwrap().push(format!("{e}"));
                            }
                        }
                    }
                }
            });

            let errs = errors.lock().unwrap();
            if !errs.is_empty() {
                return Err(ExecError::Command(errs.join("; ")));
            }

            // Find next level
            for &id in &level {
                if let Some(deps) = dependents.get(id) {
                    for &dep_id in deps {
                        if completed.contains(dep_id) {
                            continue;
                        }
                        let deg = in_degree.get_mut(dep_id).unwrap();
                        *deg -= 1;
                        if *deg == 0 {
                            queue.push_back(dep_id);
                        }
                    }
                }
            }
        }

        // Run any non-optional steps without IDs that weren't part of the DAG
        for step in steps {
            if step.meta.optional || step.id.is_some() {
                continue;
            }
            self.run_step(step, 0)?;
        }

        Ok(())
    }

    #[cfg(unix)]
    fn preflight_sudo(&self) -> Result<(), ExecError> {
        println!(
            "{}",
            style::render("<fy>sudo required - validating credentials...</f>")
        );
        let status = Command::new("sudo").arg("-v").status()?;
        if !status.success() {
            return Err(ExecError::Command("sudo authentication failed".into()));
        }
        Ok(())
    }

    /// Run a step after resolving its `depends-on` prerequisites (transitively).
    pub fn run_with_deps(&self, step: &Step) -> Result<(), ExecError> {
        let mut visited = std::collections::HashSet::new();
        self.resolve_deps(step, &mut visited)?;
        self.run_step(step, 0)
    }

    fn resolve_deps(
        &self,
        step: &Step,
        visited: &mut std::collections::HashSet<String>,
    ) -> Result<(), ExecError> {
        for dep_id in &step.depends_on {
            if visited.contains(dep_id) {
                continue;
            }
            visited.insert(dep_id.clone());
            let dep = self
                .index
                .get(dep_id)
                .ok_or_else(|| ExecError::StepNotFound(dep_id.clone()))?;
            self.resolve_deps(dep, visited)?;
            self.run_step(dep, 0)?;
        }
        Ok(())
    }

    pub fn run_step(&self, step: &Step, depth: usize) -> Result<(), ExecError> {
        let indent = "  ".repeat(depth);

        // Hard cycle limit
        if let Some(id) = &step.id {
            let mut counts = self.entry_counts.lock().unwrap();
            let count = counts.entry(id.clone()).or_insert(0);
            *count += 1;
            if *count > MAX_ENTRIES {
                return Err(ExecError::CycleDetected(step.name.clone()));
            }
        }

        if self.quiet < 1 {
            println!(
                "{indent}{}",
                style::render(&format!("<fg>-></f> <mb>{}</m>", step.name))
            );
        }

        let max_retries = step.meta.retries.or(self.config_meta.retries).unwrap_or(0);
        let mut last_err = None;

        for attempt in 0..=max_retries {
            if attempt > 0
                && let Some(delay) = step.meta.retry_delay
            {
                if !self.dry_run {
                    println!(
                        "{indent}  {}",
                        style::render(&format!("<fy>retrying in {delay}s...</f>"))
                    );
                    std::thread::sleep(std::time::Duration::from_secs_f64(delay));
                } else {
                    println!(
                        "{indent}  {}",
                        style::render(&format!(
                            "<md>[dry-run]</m> would sleep {delay}s before retry"
                        ))
                    );
                }
            }

            match self.exec_action(&step.action, &step.meta, &indent, depth) {
                Ok(code) => {
                    // Resolve handler: on-return[code] -> on-return["_"] -> on-success
                    let handler = self.resolve_handler(step, code, true);
                    if let Some(refs) = handler {
                        self.run_step_refs(refs, depth + 1)?;
                    }
                    // Run then steps
                    for child in &step.then {
                        self.run_step_ref(child, depth + 1)?;
                    }
                    return Ok(());
                }
                Err(e) => {
                    last_err = Some(e);
                }
            }
        }

        // All retries exhausted - resolve failure handler
        let err = last_err.unwrap();
        let handler = self.resolve_handler(step, -1, false);
        if let Some(refs) = handler {
            self.run_step_refs(refs, depth + 1)?;
            // Handler caught it - run then steps
            for child in &step.then {
                self.run_step_ref(child, depth + 1)?;
            }
            return Ok(());
        }

        if step.meta.fallible {
            if self.quiet < 1 {
                println!(
                    "{indent}  {}",
                    style::render(&format!("<fy>failed (fallible):</f> {err}"))
                );
            }
            return Ok(()); // don't run then
        }
        Err(err)
    }

    /// Resolve which handler to run. Returns None if no handler matches.
    fn resolve_handler<'a>(
        &self,
        step: &'a Step,
        code: i32,
        success: bool,
    ) -> Option<&'a [StepRef]> {
        // Check on-return for exact code
        if let Some(map) = &step.on_return {
            let key = code.to_string();
            if let Some(refs) = map.get(&key) {
                return Some(refs);
            }
            if let Some(refs) = map.get("_") {
                return Some(refs);
            }
        }
        // Fall back to on-success / on-failure
        if success {
            step.on_success.as_deref()
        } else {
            step.on_failure.as_deref()
        }
    }

    fn run_ref(&self, id: &str, depth: usize) -> Result<(), ExecError> {
        // StepNotFound is defensive: validate_refs catches missing IDs at parse time,
        // but this guards against a StepRef being passed in that wasn't validated.
        let step = self
            .index
            .get(id)
            .ok_or_else(|| ExecError::StepNotFound(id.into()))?;
        self.run_step(step, depth)
    }

    fn run_step_ref(&self, sr: &StepRef, depth: usize) -> Result<(), ExecError> {
        match sr {
            StepRef::Id(id) => self.run_ref(id, depth),
            StepRef::Inline(step) => self.run_step(step, depth),
        }
    }

    fn run_step_refs(&self, refs: &[StepRef], depth: usize) -> Result<(), ExecError> {
        for sr in refs {
            self.run_step_ref(sr, depth)?;
        }
        Ok(())
    }

    /// Execute an action, returning the exit code (0 for non-shell actions on success).
    /// Substitute runtime vars in a string using the current scope.
    pub fn subst(&self, s: &str) -> String {
        self.scope.lock().unwrap().substitute(s)
    }

    /// Resolve the effective ShellConfig: step-level overrides config-level, falls back to platform default.
    fn resolve_shell(&self, meta: &StepMeta) -> crate::config::ShellConfig {
        meta.shell
            .clone()
            .or_else(|| self.config_meta.shell.clone())
            .unwrap_or_default()
    }

    /// Build a Command for running a shell command string, respecting sudo and ShellConfig.
    fn shell_command(&self, cmd: &str, meta: &StepMeta, sudo: bool) -> Command {
        let shell = self.resolve_shell(meta);
        let use_sudo = sudo && cfg!(unix);
        if use_sudo {
            let mut p = Command::new("sudo");
            p.arg(&shell.cmd);
            for a in &shell.args {
                p.arg(a);
            }
            p.arg(cmd);
            p
        } else {
            let mut p = Command::new(&shell.cmd);
            for a in &shell.args {
                p.arg(a);
            }
            p.arg(cmd);
            p
        }
    }

    /// Format the shell prefix for display (dry-run, inspect).
    fn shell_prefix(&self, meta: &StepMeta, sudo: bool) -> String {
        let shell = self.resolve_shell(meta);
        let base = std::iter::once(shell.cmd.as_str())
            .chain(shell.args.iter().map(|s| s.as_str()))
            .collect::<Vec<_>>()
            .join(" ");
        if sudo { format!("sudo {base}") } else { base }
    }

    /// Apply merged env vars (global meta.env + step-level env) to a Command.
    fn apply_env(&self, proc: &mut Command, step_env: Option<&HashMap<String, String>>) {
        if let Some(global) = &self.config_meta.env {
            for (k, v) in global {
                proc.env(k, self.subst(v));
            }
        }
        if let Some(e) = step_env {
            for (k, v) in e {
                proc.env(k, self.subst(v));
            }
        }
    }

    /// Conditionally substitute: when `enabled` is true this is `subst`, else
    /// an owned copy of `s`. Used to honor per-field `expand` flags on fs
    /// actions.
    fn expand_s(&self, enabled: bool, s: &str) -> String {
        if enabled {
            self.subst(s)
        } else {
            s.to_string()
        }
    }

    fn exec_action(
        &self,
        action: &Action,
        meta: &StepMeta,
        indent: &str,
        _depth: usize,
    ) -> Result<i32, ExecError> {
        let sudo = meta.sudo || self.config_meta.sudo;
        match action {
            Action::Shell { commands, dir, env } => {
                self.exec_shell(commands, dir.as_deref(), env.as_ref(), meta, indent, sudo)
            }
            Action::Git {
                repo,
                dest,
                on_conflict,
            } => {
                self.exec_git(repo, dest, on_conflict, meta, indent)?;
                Ok(0)
            }
            Action::Fs {
                op,
                if_exists,
                if_not_exists,
            } => {
                self.exec_fs(op, if_exists.as_ref(), if_not_exists.as_ref(), indent)?;
                Ok(0)
            }
            Action::Io { op } => {
                self.exec_io(op, meta, indent)?;
                Ok(0)
            }
            Action::Var { name, source } => {
                self.exec_var(name, source, meta, indent)?;
                Ok(0)
            }
            Action::Cond { cmp, when, default } => {
                self.exec_cond(cmp, when, default.as_deref(), indent, _depth)?;
                Ok(0)
            }
            Action::Rig { file, set } => {
                self.exec_rig(file, set.as_ref(), indent, _depth)?;
                Ok(0)
            }
        }
    }

    fn maybe_print(&self, stdout: &[u8], stderr: &[u8], meta: &StepMeta) {
        let silent = if meta.silent.is_empty() {
            &self.config_meta.silent
        } else {
            &meta.silent
        };
        let suppressed = self.quiet >= 2 || self.cli_silent;
        let show_out = !suppressed && (!silent.contains(&Silent::Stdout) || self.verbose);
        let show_err = !suppressed && (!silent.contains(&Silent::Stderr) || self.verbose);
        if show_out && !stdout.is_empty() {
            print!("{}", String::from_utf8_lossy(stdout));
        }
        if show_err && !stderr.is_empty() {
            eprint!("{}", String::from_utf8_lossy(stderr));
        }
        // Always write to log file
        self.write_log_bytes(stdout);
        self.write_log_bytes(stderr);
    }

    fn write_log(&self, msg: &str) {
        use std::io::Write;
        if let Some(f) = self.log_file.lock().unwrap().as_mut() {
            let _ = writeln!(f, "{msg}");
        }
    }

    fn write_log_bytes(&self, data: &[u8]) {
        use std::io::Write;
        if !data.is_empty()
            && let Some(f) = self.log_file.lock().unwrap().as_mut()
        {
            let _ = f.write_all(data);
        }
    }

    // -- Shell --

    fn exec_shell(
        &self,
        commands: &[String],
        dir: Option<&str>,
        env: Option<&HashMap<String, String>>,
        meta: &StepMeta,
        indent: &str,
        sudo: bool,
    ) -> Result<i32, ExecError> {
        let mut last_code = 0;
        for cmd in commands {
            let cmd = self.subst(cmd);
            if self.dry_run {
                let prefix = self.shell_prefix(meta, sudo);
                println!("{indent}  [dry-run] {prefix} {cmd:?}");
                if let Some(d) = dir {
                    println!("{indent}    dir: {}", self.subst(d));
                }
                if let Some(e) = env {
                    println!("{indent}    env: {e:?}");
                }
                continue;
            }
            let mut proc = self.shell_command(&cmd, meta, sudo);
            if let Some(d) = dir {
                proc.current_dir(expand_tilde(&self.subst(d)));
            }
            self.apply_env(&mut proc, env);
            let output = proc.output()?;
            last_code = output.status.code().unwrap_or(-1);
            self.maybe_print(&output.stdout, &output.stderr, meta);
            if !output.status.success() {
                return Err(ExecError::Command(format!(
                    "command failed (exit {last_code}): {cmd}"
                )));
            }
        }
        Ok(last_code)
    }

    // -- Var --

    fn exec_var(
        &self,
        name: &str,
        source: &VarSource,
        meta: &StepMeta,
        indent: &str,
    ) -> Result<(), ExecError> {
        // Parse and validate the name.
        let vr = crate::vars::VarRef::parse(name)
            .ok_or_else(|| ExecError::Command(format!("invalid variable name: {name}")))?;
        if !vr.is_runtime_writable() {
            return Err(ExecError::Command(format!(
                "cannot assign to non-mutable variable '{}'; only @-prefixed vars are runtime-writable",
                vr.display()
            )));
        }

        let key = vr.key();

        if self.dry_run {
            let desc = match source {
                VarSource::From { from } => format!("capture stdout of {}", step_ref_label(from)),
                VarSource::To { to } => format!("feed {key} to {}", step_ref_label(to)),
                VarSource::Command { command } => format!("run `{command}`"),
                VarSource::File { file } => format!("read file {file:?}"),
            };
            println!("{indent}  [dry-run] var {key} := {desc}");
            return Ok(());
        }

        match source {
            VarSource::Command { command } => {
                let cmd = self.subst(command);
                let sudo = meta.sudo || self.config_meta.sudo;
                let mut proc = self.shell_command(&cmd, meta, sudo);
                let output = proc.output()?;
                if !output.status.success() {
                    return Err(ExecError::Command(format!("var command failed: {cmd}")));
                }
                let value = String::from_utf8_lossy(&output.stdout)
                    .trim_end_matches('\n')
                    .to_string();
                self.scope.lock().unwrap().set(&key, value.clone());
                println!(
                    "{indent}  {}",
                    style::render(&format!("<fg>set</f> <mb>{key}</m> = {value:?}"))
                );
            }
            VarSource::From { from } => {
                // Run the referenced step as a shell action and capture stdout.
                let step = resolve_step_ref(from, &self.index)
                    .map_err(|id| ExecError::StepNotFound(id.into()))?;
                let output = self.capture_step_stdout(&step)?;
                let value = output.trim_end_matches('\n').to_string();
                self.scope.lock().unwrap().set(&key, value.clone());
                println!(
                    "{indent}  {}",
                    style::render(&format!(
                        "<fg>set</f> <mb>{key}</m> \\<- {} stdout",
                        step.name
                    ))
                );
            }
            VarSource::To { to } => {
                // Feed the variable's current value as stdin to the referenced step.
                let value = self
                    .scope
                    .lock()
                    .unwrap()
                    .get(&key)
                    .map(|s| s.to_string())
                    .ok_or_else(|| ExecError::Command(format!("var '{key}' is not set")))?;
                let step = resolve_step_ref(to, &self.index)
                    .map_err(|id| ExecError::StepNotFound(id.into()))?;
                self.feed_step_stdin(&step, &value)?;
                println!(
                    "{indent}  {}",
                    style::render(&format!("<fg>fed</f> <mb>{key}</m> -> {}", step.name))
                );
            }
            VarSource::File { file } => {
                let path = crate::path::expand_tilde(&self.subst(file));
                let contents = std::fs::read_to_string(&path).map_err(|e| {
                    ExecError::Command(format!("failed to read {}: {e}", path.display()))
                })?;
                self.scope.lock().unwrap().set(&key, contents);
                println!(
                    "{indent}  {}",
                    style::render(&format!(
                        "<fg>set</f> <mb>{key}</m> \\<- {}",
                        path.display()
                    ))
                );
            }
        }
        Ok(())
    }

    fn exec_cond(
        &self,
        cmp: &str,
        when: &HashMap<String, Vec<StepRef>>,
        default: Option<&[StepRef]>,
        indent: &str,
        depth: usize,
    ) -> Result<(), ExecError> {
        let value = self.subst(cmp);

        if self.dry_run {
            println!("{indent}  [dry-run] cond: compare {cmp:?} (resolved: {value:?})");
            for (key, refs) in when {
                let labels: Vec<_> = refs.iter().map(step_ref_label).collect();
                println!("{indent}    when {key:?} -> {}", labels.join(", "));
            }
            if let Some(refs) = default {
                let labels: Vec<_> = refs.iter().map(step_ref_label).collect();
                println!("{indent}    default -> {}", labels.join(", "));
            }
            return Ok(());
        }

        if let Some(refs) = when.get(&value) {
            self.run_step_refs(refs, depth + 1)?;
        } else if let Some(refs) = default {
            self.run_step_refs(refs, depth + 1)?;
        }
        Ok(())
    }

    fn exec_rig(
        &self,
        file: &str,
        set: Option<&HashMap<String, String>>,
        indent: &str,
        depth: usize,
    ) -> Result<(), ExecError> {
        let file = self.subst(file);
        // Build cli_vars: parent scope values + set overrides (substituted)
        let mut cli_vars: HashMap<String, String> = self.scope.lock().unwrap().all_values();
        if let Some(s) = set {
            for (k, v) in s {
                cli_vars.insert(k.clone(), self.subst(v));
            }
        }

        if self.dry_run {
            println!("{indent}  [dry-run] rig {file:?}");
            if let Some(s) = set {
                for (k, v) in s {
                    println!("{indent}    set {k} = {v}");
                }
            }
            return Ok(());
        }

        // Parse the sub-config
        let cfg = crate::config::parse_config(&file, &cli_vars, false)
            .map_err(|e| ExecError::Command(format!("rig sub-config error: {e}")))?;

        // Build scope for sub-config
        let sub_scope = crate::config::build_scope(&cfg, &cli_vars);
        let sub_index = crate::config::build_step_index(&cfg);
        let sub_runner = Runner::new(
            sub_index,
            self.dry_run,
            self.verbose,
            cfg.meta.clone(),
            sub_scope,
        );

        if self.quiet < 1 {
            println!(
                "{indent}  {}",
                style::render(&format!("<fc>rig:</f> <mb>{}</m>", cfg.name))
            );
        }
        for step in &cfg.steps {
            if step.meta.optional {
                continue;
            }
            sub_runner.run_step(step, depth + 1)?;
        }
        Ok(())
    }

    /// Run a step whose action is Shell and capture its stdout.
    fn capture_step_stdout(&self, step: &Step) -> Result<String, ExecError> {
        match &step.action {
            Action::Shell { commands, dir, env } => {
                let sudo = step.meta.sudo || self.config_meta.sudo;
                let mut acc = String::new();
                for cmd in commands {
                    let cmd = self.subst(cmd);
                    let mut proc = self.shell_command(&cmd, &step.meta, sudo);
                    if let Some(d) = dir {
                        proc.current_dir(crate::path::expand_tilde(&self.subst(d)));
                    }
                    self.apply_env(&mut proc, env.as_ref());
                    let output = proc.output()?;
                    if !output.status.success() {
                        return Err(ExecError::Command(format!(
                            "captured step failed: {}",
                            step.name
                        )));
                    }
                    acc.push_str(&String::from_utf8_lossy(&output.stdout));
                }
                Ok(acc)
            }
            _ => Err(ExecError::Command(format!(
                "var from: step '{}' must be shell action",
                step.name
            ))),
        }
    }

    /// Feed a string to a shell step's stdin.
    fn feed_step_stdin(&self, step: &Step, input: &str) -> Result<(), ExecError> {
        match &step.action {
            Action::Shell { commands, dir, env } => {
                let sudo = step.meta.sudo || self.config_meta.sudo;
                for cmd in commands {
                    let cmd = self.subst(cmd);
                    use std::io::Write;
                    use std::process::Stdio;
                    let mut proc = self.shell_command(&cmd, &step.meta, sudo);
                    if let Some(d) = dir {
                        proc.current_dir(crate::path::expand_tilde(&self.subst(d)));
                    }
                    self.apply_env(&mut proc, env.as_ref());
                    let mut child = proc
                        .stdin(Stdio::piped())
                        .stdout(Stdio::piped())
                        .stderr(Stdio::piped())
                        .spawn()?;
                    if let Some(mut stdin) = child.stdin.take() {
                        stdin.write_all(input.as_bytes())?;
                    }
                    let output = child.wait_with_output()?;
                    self.maybe_print(&output.stdout, &output.stderr, &step.meta);
                    if !output.status.success() {
                        return Err(ExecError::Command(format!(
                            "fed step failed: {}",
                            step.name
                        )));
                    }
                }
                Ok(())
            }
            _ => Err(ExecError::Command(format!(
                "var to: step '{}' must be shell action",
                step.name
            ))),
        }
    }

    // -- IO --

    fn exec_io(&self, op: &IoOp, _meta: &StepMeta, indent: &str) -> Result<(), ExecError> {
        match op {
            IoOp::Write {
                level,
                message,
                markup,
            } => {
                self.io_write(level, message, *markup, indent);
                Ok(())
            }
            IoOp::Read {
                read,
                prompt,
                default,
                secret,
            } => self.io_read(read, prompt.as_deref(), default.as_deref(), *secret, indent),
        }
    }

    fn io_write(&self, level: &IoLevel, message: &str, markup: bool, indent: &str) {
        // Use the colorized substitution so unresolved @vars stand out in the terminal.
        let message_display = self.scope.lock().unwrap().substitute_display(message);
        // For log file and fallback text, use plain substitution.
        let message_plain = self.subst(message);
        let text = if markup {
            style::render(&message_display)
        } else {
            // Non-markup io still benefits from highlighting unresolved vars, but we don't
            // want to render markup tags inside arbitrary user text. Use plain here.
            message_plain.clone()
        };
        let line = match level {
            IoLevel::Log => format!("{indent}  {text}"),
            IoLevel::Info => format!(
                "{indent}  {}",
                style::render(&format!("<fc>info:</f> {text}"))
            ),
            IoLevel::Warn => format!(
                "{indent}  {}",
                style::render(&format!("<fy>warn:</f> {text}"))
            ),
            IoLevel::Error => format!(
                "{indent}  {}",
                style::render(&format!("<fr>error:</f> {text}"))
            ),
        };
        if self.quiet < 2 {
            println!("{line}");
        }
        let plain = if markup {
            message.to_string()
        } else {
            text.clone()
        };
        let prefix = match level {
            IoLevel::Log => "LOG",
            IoLevel::Info => "INFO",
            IoLevel::Warn => "WARN",
            IoLevel::Error => "ERROR",
        };
        self.write_log(&format!("[{prefix}] {plain}"));
    }

    fn io_read(
        &self,
        read: &str,
        prompt: Option<&str>,
        default: Option<&str>,
        secret: bool,
        indent: &str,
    ) -> Result<(), ExecError> {
        let vr = crate::vars::VarRef::parse(read)
            .ok_or_else(|| ExecError::Command(format!("invalid io read target: {read}")))?;
        if !vr.is_runtime_writable() {
            return Err(ExecError::Command(format!(
                "io read target '{}' is not runtime-writable",
                vr.display()
            )));
        }
        let key = vr.key();

        if self.dry_run {
            if let Some(d) = default {
                self.scope.lock().unwrap().set(&key, d.to_string());
                println!("{indent}  [dry-run] read {key} = {d:?} (default)");
            } else {
                println!("{indent}  [dry-run] read {key} (no input in dry-run; unset)");
            }
            return Ok(());
        }

        // Render prompt.
        let prompt_resolved = prompt.map(|p| self.subst(p));
        let prompt_text = prompt_resolved.as_deref().unwrap_or("");

        let line = if secret {
            use std::io::Write;
            if !prompt_text.is_empty() {
                print!("{prompt_text}");
                std::io::stdout().flush().ok();
            }
            let config = rpassword::ConfigBuilder::new()
                .password_feedback_mask('*')
                .build();
            rpassword::read_password_with_config(config)
                .map_err(|e| ExecError::Command(format!("stdin read failed: {e}")))?
        } else {
            use std::io::Write;
            if !prompt_text.is_empty() {
                print!("{prompt_text}");
                std::io::stdout().flush().ok();
            }
            let mut buf = String::new();
            std::io::stdin()
                .read_line(&mut buf)
                .map_err(|e| ExecError::Command(format!("stdin read failed: {e}")))?;
            buf.trim_end_matches(['\n', '\r']).to_string()
        };

        let value = if line.is_empty() {
            match default {
                Some(d) => d.to_string(),
                None => {
                    println!(
                        "{indent}  {}",
                        style::render(&format!("<fy>read {key} unset (no input, no default)</f>"))
                    );
                    return Ok(());
                }
            }
        } else {
            line
        };

        let display = if secret {
            "****".to_string()
        } else {
            format!("{value:?}")
        };
        self.scope.lock().unwrap().set(&key, value);
        println!(
            "{indent}  {}",
            style::render(&format!("<fg>read</f> <mb>{key}</m> = {display}"))
        );
        Ok(())
    }

    // -- Git --

    fn exec_git(
        &self,
        repo: &str,
        dest: &str,
        on_conflict: &GitOnConflict,
        meta: &StepMeta,
        indent: &str,
    ) -> Result<(), ExecError> {
        let repo = self.subst(repo);
        let dest = self.subst(dest);
        let dest_path = expand_tilde(&dest);
        let exists = dest_path.exists();

        if self.dry_run {
            if exists {
                println!(
                    "{indent}  [dry-run] dest {} exists -> {on_conflict:?}",
                    dest_path.display()
                );
            } else {
                println!(
                    "{indent}  [dry-run] git clone {repo} {}",
                    dest_path.display()
                );
            }
            return Ok(());
        }

        if !exists {
            let output = Command::new("git")
                .args(["clone", &repo, &dest_path.to_string_lossy()])
                .output()?;
            self.maybe_print(&output.stdout, &output.stderr, meta);
            if !output.status.success() {
                return Err(ExecError::Command(format!(
                    "git clone failed ({})",
                    output.status
                )));
            }
            return Ok(());
        }

        match on_conflict {
            GitOnConflict::Skip => println!(
                "{indent}  {}",
                style::render(&format!(
                    "<fy>skipped (exists):</f> {}",
                    dest_path.display()
                ))
            ),
            GitOnConflict::Pull => {
                let output = Command::new("git")
                    .args(["-C", &dest_path.to_string_lossy(), "pull"])
                    .output()?;
                self.maybe_print(&output.stdout, &output.stderr, meta);
                if !output.status.success() {
                    return Err(ExecError::Command(format!(
                        "git pull failed ({})",
                        output.status
                    )));
                }
            }
            GitOnConflict::Fail => {
                return Err(ExecError::Command(format!(
                    "dest already exists: {}",
                    dest_path.display()
                )));
            }
        }
        Ok(())
    }

    // -- FS --

    fn exec_fs(
        &self,
        op: &FsOp,
        if_exists: Option<&Condition>,
        if_not_exists: Option<&Condition>,
        indent: &str,
    ) -> Result<(), ExecError> {
        match op {
            FsOp::Create {
                path,
                recurse,
                content,
                expand,
            } => self.fs_create(
                path,
                *recurse,
                content.as_deref(),
                *expand,
                if_exists,
                indent,
            ),
            FsOp::Symlink { from, to, expand } => {
                self.fs_symlink(from, to, *expand, if_exists, indent)
            }
            FsOp::Copy { from, to, expand } => {
                self.fs_copy(from, to, *expand, if_exists, if_not_exists, indent)
            }
            FsOp::Move { from, to, expand } => {
                self.fs_move(from, to, *expand, if_exists, if_not_exists, indent)
            }
            FsOp::Delete {
                path,
                recurse,
                expand,
            } => self.fs_delete(path, *recurse, *expand, if_not_exists, indent),
        }
    }

    fn fs_create(
        &self,
        path: &[String],
        recurse: bool,
        content: Option<&str>,
        expand: ExpandFlags,
        if_exists: Option<&Condition>,
        indent: &str,
    ) -> Result<(), ExecError> {
        for p in path {
            let p = self.expand_s(expand.path, p);
            let full = expand_tilde(&p);
            let is_dir = p.ends_with('/');
            // Inline `content` rendering still follows the legacy behavior
            // (always substituted); bundles never go through this branch.
            let content_resolved = content.map(|c| self.subst(c));
            let content = content_resolved.as_deref();
            if self.dry_run {
                let kind = if is_dir { "dir" } else { "file" };
                println!("{indent}  [dry-run] create {kind}: {}", full.display());
                if let Some(c) = content {
                    println!("{indent}    content: {c:?}");
                }
                continue;
            }
            let mut append_mode = false;
            if full.exists() {
                match self.handle_condition(if_exists, indent)? {
                    CondResult::Skip => continue,
                    CondResult::Proceed => {}
                    CondResult::Append => {
                        if is_dir {
                            return Err(ExecError::Command(format!(
                                "cannot append to directory: {}",
                                full.display()
                            )));
                        }
                        append_mode = true;
                    }
                    CondResult::Panic => {
                        return Err(ExecError::Command(format!(
                            "already exists: {}",
                            full.display()
                        )));
                    }
                }
            }
            if is_dir {
                if recurse {
                    std::fs::create_dir_all(&full)?;
                } else {
                    std::fs::create_dir(&full)?;
                }
            } else {
                if recurse && let Some(parent) = full.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                if let Some(c) = content {
                    if append_mode {
                        use std::io::Write;
                        let mut f = std::fs::OpenOptions::new().append(true).open(&full)?;
                        f.write_all(c.as_bytes())?;
                    } else {
                        std::fs::write(&full, c)?;
                    }
                } else if !append_mode {
                    std::fs::File::create(&full)?;
                }
            }
            let verb = if append_mode {
                "appended to"
            } else {
                "created"
            };
            if self.quiet < 1 {
                println!(
                    "{indent}  {}",
                    style::render(&format!("<fg>{verb}:</f> {}", full.display()))
                );
            }
        }
        Ok(())
    }

    fn fs_symlink(
        &self,
        from: &str,
        to: &str,
        expand: ExpandFlags,
        if_exists: Option<&Condition>,
        indent: &str,
    ) -> Result<(), ExecError> {
        let from = self.expand_s(expand.from, from);
        let to = self.expand_s(expand.to, to);
        let src = expand_tilde(&from);
        let dst = expand_tilde(&to);
        if self.dry_run {
            println!(
                "{indent}  [dry-run] symlink {} -> {}",
                src.display(),
                dst.display()
            );
            return Ok(());
        }
        if dst.exists() || dst.is_symlink() {
            match self.handle_condition(if_exists, indent)? {
                CondResult::Skip => return Ok(()),
                CondResult::Proceed => {
                    std::fs::remove_file(&dst).or_else(|_| std::fs::remove_dir_all(&dst))?;
                }
                CondResult::Append => {
                    return Err(ExecError::Command(
                        "append not supported for symlink".into(),
                    ));
                }
                CondResult::Panic => {
                    return Err(ExecError::Command(format!(
                        "already exists: {}",
                        dst.display()
                    )));
                }
            }
        }
        #[cfg(unix)]
        std::os::unix::fs::symlink(&src, &dst)?;
        #[cfg(windows)]
        {
            let res = if src.is_dir() {
                std::os::windows::fs::symlink_dir(&src, &dst)
            } else {
                std::os::windows::fs::symlink_file(&src, &dst)
            };
            res.map_err(|e| {
                if e.raw_os_error() == Some(1314) {
                    ExecError::Command(format!(
                        "symlink failed: insufficient privileges. Run as Administrator or enable Developer Mode.\n  {} -> {}",
                        src.display(), dst.display()
                    ))
                } else {
                    ExecError::Io(e)
                }
            })?;
        }
        println!(
            "{indent}  {}",
            style::render(&format!(
                "<fg>symlinked</f> {} -> {}",
                src.display(),
                dst.display()
            ))
        );
        Ok(())
    }

    fn fs_copy(
        &self,
        from: &str,
        to: &str,
        expand: ExpandFlags,
        if_exists: Option<&Condition>,
        if_not_exists: Option<&Condition>,
        indent: &str,
    ) -> Result<(), ExecError> {
        let from = self.expand_s(expand.from, from);
        let to = self.expand_s(expand.to, to);
        let src = expand_tilde(&from);
        let dst = expand_tilde(&to);
        if self.dry_run {
            let mode = self.copy_mode_label(&src, expand.contents);
            println!(
                "{indent}  [dry-run] copy {} -> {} {mode}",
                src.display(),
                dst.display()
            );
            return Ok(());
        }
        if !src.exists() {
            return self.handle_not_exists(if_not_exists, &src, indent);
        }
        let mut append_mode = false;
        if dst.exists() {
            match self.handle_condition(if_exists, indent)? {
                CondResult::Skip => return Ok(()),
                CondResult::Proceed => {}
                CondResult::Append => {
                    append_mode = true;
                }
                CondResult::Panic => {
                    return Err(ExecError::Command(format!(
                        "already exists: {}",
                        dst.display()
                    )));
                }
            }
        }

        // Determine whether to render `{{...}}` in file contents.
        //
        // Rules (see Task 8 plan):
        // - Outside a bundle: never render (legacy byte-exact copy).
        // - Inside a bundle, src inside the staging root:
        //     - binary-glob match  → byte-exact
        //     - `expand.contents`  → render (explicit user request)
        //     - otherwise          → render (the point of bundles is templated content)
        // - Inside a bundle, src outside the staging root: byte-exact (treat as
        //   a normal filesystem copy).
        let render_contents = self.should_render_contents(&src, expand.contents);

        if append_mode {
            use std::io::Write;
            let bytes = self.read_for_copy(&src, render_contents)?;
            let mut f = std::fs::OpenOptions::new().append(true).open(&dst)?;
            f.write_all(&bytes)?;
            let tag = if render_contents { " (rendered)" } else { "" };
            println!(
                "{indent}  {}",
                style::render(&format!(
                    "<fg>appended{tag}</f> {} -> {}",
                    src.display(),
                    dst.display()
                ))
            );
        } else if render_contents {
            let bytes = self.read_for_copy(&src, true)?;
            std::fs::write(&dst, &bytes)?;
            println!(
                "{indent}  {}",
                style::render(&format!(
                    "<fg>copied (rendered)</f> {} -> {}",
                    src.display(),
                    dst.display()
                ))
            );
        } else {
            std::fs::copy(&src, &dst)?;
            println!(
                "{indent}  {}",
                style::render(&format!(
                    "<fg>copied</f> {} -> {}",
                    src.display(),
                    dst.display()
                ))
            );
        }
        Ok(())
    }

    /// Decide whether to apply variable substitution to the contents of `src`
    /// during an `fs.copy`.
    ///
    /// See the rule table in `fs_copy` for the semantics.
    fn should_render_contents(&self, src: &std::path::Path, contents_flag: bool) -> bool {
        let Some(ctx) = self.bundle.as_ref() else {
            return false; // No bundle → legacy byte-exact copy.
        };
        let Some(rel) = src.strip_prefix(&ctx.root).ok() else {
            return false; // src lives outside the bundle; treat as normal fs.
        };
        if ctx.binary.matches(rel) {
            return false;
        }
        // Inside the bundle + not binary → render. `contents_flag` can only
        // force it on (the default is already "yes" inside a bundle).
        // When contents_flag is explicitly false via `expand: false` shorthand
        // or `expand: {contents: false}` this still returns true unless the
        // file is binary-matched; users who need byte-exact bundles should
        // list the path in `bundle.binary`.
        let _ = contents_flag;
        true
    }

    /// Read `src` as bytes, optionally rendering `{{...}}` through the current
    /// scope. When rendering is requested but the file is not valid UTF-8,
    /// we fall back to raw bytes (prevents corrupting a file that slipped
    /// through the binary-glob filter).
    fn read_for_copy(&self, src: &std::path::Path, render: bool) -> Result<Vec<u8>, ExecError> {
        let raw = std::fs::read(src)?;
        if !render {
            return Ok(raw);
        }
        match std::str::from_utf8(&raw) {
            Ok(text) => Ok(self.subst(text).into_bytes()),
            Err(_) => Ok(raw), // not UTF-8; safest to copy verbatim
        }
    }

    /// Short dry-run label describing how a copy will proceed (rendered vs
    /// byte-exact) — used only in `[dry-run]` output.
    fn copy_mode_label(&self, src: &std::path::Path, contents_flag: bool) -> &'static str {
        if self.should_render_contents(src, contents_flag) {
            "(rendered)"
        } else {
            ""
        }
    }

    fn fs_move(
        &self,
        from: &str,
        to: &str,
        expand: ExpandFlags,
        if_exists: Option<&Condition>,
        if_not_exists: Option<&Condition>,
        indent: &str,
    ) -> Result<(), ExecError> {
        let from = self.expand_s(expand.from, from);
        let to = self.expand_s(expand.to, to);
        let src = expand_tilde(&from);
        let dst = expand_tilde(&to);
        if self.dry_run {
            println!(
                "{indent}  [dry-run] move {} -> {}",
                src.display(),
                dst.display()
            );
            return Ok(());
        }
        if !src.exists() {
            return self.handle_not_exists(if_not_exists, &src, indent);
        }
        if dst.exists() {
            match self.handle_condition(if_exists, indent)? {
                CondResult::Skip => return Ok(()),
                CondResult::Proceed => {
                    std::fs::remove_file(&dst).or_else(|_| std::fs::remove_dir_all(&dst))?;
                }
                CondResult::Append => {
                    return Err(ExecError::Command("append not supported for move".into()));
                }
                CondResult::Panic => {
                    return Err(ExecError::Command(format!(
                        "already exists: {}",
                        dst.display()
                    )));
                }
            }
        }
        std::fs::rename(&src, &dst)?;
        println!(
            "{indent}  {}",
            style::render(&format!(
                "<fg>moved</f> {} -> {}",
                src.display(),
                dst.display()
            ))
        );
        Ok(())
    }

    fn fs_delete(
        &self,
        path: &[String],
        recurse: bool,
        expand: ExpandFlags,
        if_not_exists: Option<&Condition>,
        indent: &str,
    ) -> Result<(), ExecError> {
        for p in path {
            let p = self.expand_s(expand.path, p);
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
                if recurse {
                    std::fs::remove_dir_all(&full)?;
                } else {
                    std::fs::remove_dir(&full)?;
                }
            } else {
                std::fs::remove_file(&full)?;
            }
            println!(
                "{indent}  {}",
                style::render(&format!("<fg>deleted:</f> {}", full.display()))
            );
        }
        Ok(())
    }

    // -- Condition helpers --

    fn handle_condition(
        &self,
        cond: Option<&Condition>,
        _indent: &str,
    ) -> Result<CondResult, ExecError> {
        match cond {
            None | Some(Condition::Action(ConditionAction::Panic)) => Ok(CondResult::Panic),
            Some(Condition::Action(ConditionAction::Skip)) => Ok(CondResult::Skip),
            Some(Condition::Action(ConditionAction::Overwrite)) => Ok(CondResult::Proceed),
            Some(Condition::Action(ConditionAction::Append)) => Ok(CondResult::Append),
            Some(Condition::Execute { execute }) => {
                self.run_step_ref(execute, 0)?;
                Ok(CondResult::Skip)
            }
        }
    }

    fn handle_not_exists(
        &self,
        cond: Option<&Condition>,
        path: &std::path::Path,
        _indent: &str,
    ) -> Result<(), ExecError> {
        match cond {
            None | Some(Condition::Action(ConditionAction::Panic)) => Err(ExecError::Command(
                format!("does not exist: {}", path.display()),
            )),
            Some(Condition::Action(ConditionAction::Skip)) => Ok(()),
            Some(Condition::Execute { execute }) => self.run_step_ref(execute, 0),
            _ => Err(ExecError::Command(format!(
                "does not exist: {}",
                path.display()
            ))),
        }
    }

    // -- Dry-run audit --

    pub fn dry_run_audit(&self, config: &Config, steps: &[Step]) {
        // Top-level config info
        println!(
            "{}",
            style::render(&format!("<mb>version:</m> {}", config.version))
        );
        if self.verbose
            && let Some(desc) = &config.description
        {
            println!("{}", style::render(&format!("<md>{desc}</m>")));
        }
        if let Some(r) = config.meta.retries {
            println!("{}", style::render(&format!("<mb>retries:</m> {r}")));
        }
        if let Some(d) = config.meta.retry_delay {
            println!("{}", style::render(&format!("<mb>retry-delay:</m> {d}s")));
        }
        if config.meta.sudo {
            println!("{}", style::render("<mb>sudo:</m> true"));
        }
        if !config.meta.silent.is_empty() {
            let s: Vec<_> = config
                .meta
                .silent
                .iter()
                .map(|s| format!("{s:?}").to_lowercase())
                .collect();
            println!(
                "{}",
                style::render(&format!("<mb>silent:</m> {}", s.join(", ")))
            );
        }
        if let Some(log) = &config.meta.log {
            println!("{}", style::render(&format!("<mb>log:</m> {log}")));
        }

        let optional_count = steps.iter().filter(|s| s.meta.optional).count();
        let fallible_count = steps.iter().filter(|s| s.meta.fallible).count();
        let sudo_count = steps.iter().filter(|s| s.meta.sudo).count();
        let mut counts = format!(
            "{} steps, {} optional, {} fallible",
            steps.len(),
            optional_count,
            fallible_count
        );
        if sudo_count > 0 {
            counts.push_str(&format!(", {} sudo", sudo_count));
        }
        println!("{}", style::render(&format!("<md>({counts})</m>\n")));

        for step in steps {
            self.audit_step(step, 0);
        }
        println!("{}", style::render(&format!("<mb>Summary:</m> {counts}")));
    }

    fn audit_step(&self, step: &Step, depth: usize) {
        let indent = "  ".repeat(depth);
        let mut header = format!("{indent}<fg>-></f> <mb>{}</m>", step.name);
        if let Some(id) = &step.id {
            header.push_str(&format!(" <md>(id: <fc>{id}</f>)</m>"));
        }
        let mut flags = Vec::new();
        if step.meta.optional {
            flags.push("<fy>optional</f>".to_string());
        }
        if step.meta.fallible {
            flags.push("<fy>fallible</f>".to_string());
        }
        if step.meta.sudo {
            flags.push("<fy>sudo</f>".to_string());
        }
        if !step.meta.silent.is_empty() {
            let s: Vec<_> = step
                .meta
                .silent
                .iter()
                .map(|s| format!("{s:?}").to_lowercase())
                .collect();
            flags.push(format!("<fy>silent: {}</f>", s.join(", ")));
        }
        if let Some(r) = step.meta.retries {
            flags.push(format!("<fy>retries: {r}</f>"));
        }
        if let Some(d) = step.meta.retry_delay {
            flags.push(format!("<fy>retry-delay: {d}s</f>"));
        }
        for f in &flags {
            header.push_str(&format!(" [{f}]"));
        }
        println!("{}", style::render(&header));
        if self.verbose
            && let Some(desc) = &step.description
        {
            println!("{indent}  {}", style::render(&format!("<md>{desc}</m>")));
        }

        let ai = format!("{indent}    ");
        match &step.action {
            Action::Shell { commands, dir, env } => {
                let sudo = step.meta.sudo || self.config_meta.sudo;
                let prefix = self.shell_prefix(&step.meta, sudo);
                for cmd in commands {
                    println!(
                        "{ai}{}",
                        style::render(&format!("<md>{prefix}</m> {cmd:?}"))
                    );
                }
                if let Some(d) = dir {
                    println!("{ai}{}", style::render(&format!("<md>dir:</m> {d}")));
                }
                if let Some(e) = env {
                    for (k, v) in e {
                        println!("{ai}{}", style::render(&format!("<md>env:</m> {k}={v}")));
                    }
                }
            }
            Action::Git {
                repo,
                dest,
                on_conflict,
            } => {
                println!(
                    "{ai}{}",
                    style::render(&format!("<md>git clone</m> {repo} -> {dest}"))
                );
                println!(
                    "{ai}{}",
                    style::render(&format!("<md>on-conflict:</m> {on_conflict:?}"))
                );
            }
            Action::Fs {
                op,
                if_exists,
                if_not_exists,
            } => {
                match op {
                    FsOp::Create {
                        path,
                        recurse,
                        content,
                        expand,
                    } => {
                        for p in path {
                            let kind = if p.ends_with('/') { "dir" } else { "file" };
                            println!(
                                "{ai}{}",
                                style::render(&format!("<md>create {kind}:</m> {p}"))
                            );
                        }
                        if *recurse {
                            println!("{ai}{}", style::render("<md>recurse:</m> true"));
                        }
                        if let Some(c) = content {
                            println!("{ai}{}", style::render(&format!("<md>content:</m> {c:?}")));
                        }
                        if let Some(label) = expand_label(expand) {
                            println!("{ai}{}", style::render(&format!("<md>expand:</m> {label}")));
                        }
                    }
                    FsOp::Symlink { from, to, expand } => {
                        println!(
                            "{ai}{}",
                            style::render(&format!("<md>symlink</m> {from} -> {to}"))
                        );
                        if let Some(label) = expand_label(expand) {
                            println!("{ai}{}", style::render(&format!("<md>expand:</m> {label}")));
                        }
                    }
                    FsOp::Copy { from, to, expand } => {
                        println!(
                            "{ai}{}",
                            style::render(&format!("<md>copy</m> {from} -> {to}"))
                        );
                        if let Some(label) = expand_label(expand) {
                            println!("{ai}{}", style::render(&format!("<md>expand:</m> {label}")));
                        }
                    }
                    FsOp::Move { from, to, expand } => {
                        println!(
                            "{ai}{}",
                            style::render(&format!("<md>move</m> {from} -> {to}"))
                        );
                        if let Some(label) = expand_label(expand) {
                            println!("{ai}{}", style::render(&format!("<md>expand:</m> {label}")));
                        }
                    }
                    FsOp::Delete {
                        path,
                        recurse,
                        expand,
                    } => {
                        for p in path {
                            println!("{ai}{}", style::render(&format!("<md>delete:</m> {p}")));
                        }
                        if *recurse {
                            println!("{ai}{}", style::render("<md>recurse:</m> true"));
                        }
                        if let Some(label) = expand_label(expand) {
                            println!("{ai}{}", style::render(&format!("<md>expand:</m> {label}")));
                        }
                    }
                }
                if let Some(c) = if_exists {
                    println!(
                        "{ai}{}",
                        style::render(&format!("<md>if-exists:</m> {}", condition_label(c)))
                    );
                }
                if let Some(c) = if_not_exists {
                    println!(
                        "{ai}{}",
                        style::render(&format!("<md>if-not-exists:</m> {}", condition_label(c)))
                    );
                }
            }
            Action::Io { op } => match op {
                IoOp::Write {
                    level,
                    message,
                    markup,
                } => {
                    let ml = if *markup { " [markup]" } else { "" };
                    println!(
                        "{ai}{}",
                        style::render(&format!("<md>{level:?}:</m> {message:?}{ml}"))
                    );
                }
                IoOp::Read {
                    read,
                    prompt,
                    default,
                    secret,
                } => {
                    let extras = {
                        let mut v = Vec::new();
                        if let Some(p) = prompt {
                            v.push(format!("prompt: {p:?}"));
                        }
                        if let Some(d) = default {
                            v.push(format!("default: {d:?}"));
                        }
                        if *secret {
                            v.push("secret".to_string());
                        }
                        if v.is_empty() {
                            String::new()
                        } else {
                            format!(" ({})", v.join(", "))
                        }
                    };
                    println!(
                        "{ai}{}",
                        style::render(&format!("<md>read:</m> {read}{extras}"))
                    );
                }
            },
            Action::Var { name, source } => match source {
                VarSource::From { from } => println!(
                    "{ai}{}",
                    style::render(&format!("<md>var {name} \\<-</m> {}", step_ref_label(from)))
                ),
                VarSource::To { to } => println!(
                    "{ai}{}",
                    style::render(&format!("<md>var {name} -></m> {}", step_ref_label(to)))
                ),
                VarSource::Command { command } => println!(
                    "{ai}{}",
                    style::render(&format!("<md>var {name} :=</m> {command:?}"))
                ),
                VarSource::File { file } => println!(
                    "{ai}{}",
                    style::render(&format!("<md>var {name} \\<- file</m> {file:?}"))
                ),
            },
            Action::Cond { cmp, when, default } => {
                println!("{ai}{}", style::render(&format!("<md>cond:</m> {cmp:?}")));
                for (key, refs) in when {
                    println!(
                        "{ai}  {}",
                        style::render(&format!("<fc>{key:?}</f> -> {}", step_refs_label(refs)))
                    );
                }
                if let Some(refs) = default {
                    println!(
                        "{ai}  {}",
                        style::render(&format!("<fy>default</f> -> {}", step_refs_label(refs)))
                    );
                }
            }
            Action::Rig { file, set } => {
                println!("{ai}{}", style::render(&format!("<md>rig:</m> {file:?}")));
                if let Some(s) = set {
                    for (k, v) in s {
                        println!(
                            "{ai}  {}",
                            style::render(&format!("<md>set</m> {k} = {v:?}"))
                        );
                    }
                }
            }
        }

        // on-success / on-failure / on-return
        if let Some(refs) = &step.on_success {
            println!(
                "{ai}{}",
                style::render(&format!("<md>on-success:</m> {}", step_refs_label(refs)))
            );
        }
        if let Some(refs) = &step.on_failure {
            println!(
                "{ai}{}",
                style::render(&format!("<md>on-failure:</m> {}", step_refs_label(refs)))
            );
        }
        if let Some(map) = &step.on_return {
            println!("{ai}{}", style::render("<md>on-return:</m>"));
            for (code, refs) in map {
                println!(
                    "{ai}  {}",
                    style::render(&format!("{code} -> <fc>{}</f>", step_refs_label(refs)))
                );
            }
        }

        // then
        if !step.then.is_empty() {
            println!("{ai}{}", style::render("<md>then:</m>"));
            for child in &step.then {
                match child {
                    StepRef::Id(id) => {
                        println!("{ai}  {}", style::render(&format!("-> <fc>{id}</f>")))
                    }
                    StepRef::Inline(s) => self.audit_step(s, depth + 1),
                }
            }
        }
        println!();
    }
}

// -- Helpers --

enum CondResult {
    Skip,
    Proceed,
    Append,
    Panic,
}

fn step_ref_label(sr: &StepRef) -> String {
    match sr {
        StepRef::Id(id) => id.clone(),
        StepRef::Inline(s) => format!("[inline: {}]", s.name),
    }
}

/// Resolve a StepRef to an owned Step, looking up ID refs in the index.
fn resolve_step_ref<'a>(
    sr: &'a StepRef,
    index: &'a HashMap<String, Step>,
) -> Result<std::borrow::Cow<'a, Step>, &'a str> {
    match sr {
        StepRef::Id(id) => index
            .get(id)
            .map(std::borrow::Cow::Borrowed)
            .ok_or(id.as_str()),
        StepRef::Inline(s) => Ok(std::borrow::Cow::Borrowed(s)),
    }
}

fn step_refs_label(refs: &[StepRef]) -> String {
    refs.iter()
        .map(step_ref_label)
        .collect::<Vec<_>>()
        .join(", ")
}

fn condition_label(c: &Condition) -> String {
    match c {
        Condition::Action(a) => format!("{a:?}").to_lowercase(),
        Condition::Execute { execute } => format!("execute({})", step_ref_label(execute)),
    }
}

/// Format an ExpandFlags into a compact description suitable for --dry-run /
/// --describe output. Returns None when the flags match the PATHS default
/// (paths substituted, contents byte-exact), so we don't clutter the audit
/// with the implicit common case.
fn expand_label(flags: &ExpandFlags) -> Option<String> {
    if *flags == ExpandFlags::PATHS {
        return None;
    }
    if *flags == ExpandFlags::NONE {
        return Some("none (byte-exact)".into());
    }
    if *flags == ExpandFlags::ALL {
        return Some("all".into());
    }
    let mut parts = Vec::new();
    if flags.from {
        parts.push("from");
    }
    if flags.to {
        parts.push("to");
    }
    if flags.path {
        parts.push("path");
    }
    if flags.contents {
        parts.push("contents");
    }
    Some(parts.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn runner(index: HashMap<String, Step>) -> Runner {
        Runner::new(
            index,
            false,
            false,
            Meta::default(),
            crate::vars::Scope::default(),
        )
    }

    fn dry_runner() -> Runner {
        Runner::new(
            HashMap::new(),
            true,
            false,
            Meta::default(),
            crate::vars::Scope::default(),
        )
    }

    fn shell_step(commands: Vec<&str>, dir: Option<&str>) -> Step {
        Step {
            id: None,
            name: "test".into(),
            description: None,
            action: Action::Shell {
                commands: commands.into_iter().map(String::from).collect(),
                dir: dir.map(String::from),
                env: None,
            },
            on_success: None,
            on_failure: None,
            on_return: None,
            then: vec![],
            depends_on: vec![],
            meta: StepMeta::default(),
        }
    }

    #[test]
    fn shell_runs_command() {
        let dir = tempfile::tempdir().unwrap();
        let step = shell_step(
            vec!["echo hi > out.txt"],
            Some(dir.path().to_str().unwrap()),
        );
        runner(HashMap::new()).run_step(&step, 0).unwrap();
        assert!(dir.path().join("out.txt").exists());
    }

    #[test]
    fn shell_dry_run() {
        let dir = tempfile::tempdir().unwrap();
        let step = shell_step(
            vec!["echo hi > out.txt"],
            Some(dir.path().to_str().unwrap()),
        );
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
            id: None,
            name: "create".into(),
            description: None,
            action: Action::Fs {
                op: FsOp::Create {
                    path: vec![path_str],
                    recurse: false,
                    content: None,
                    expand: ExpandFlags::NONE,
                },
                if_exists: None,
                if_not_exists: None,
            },
            on_success: None,
            on_failure: None,
            on_return: None,
            then: vec![],
            depends_on: vec![],
            meta: StepMeta::default(),
        };
        runner(HashMap::new()).run_step(&step, 0).unwrap();
        assert!(target.is_dir());
    }

    #[test]
    fn fs_create_file() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("newfile.txt");
        let step = Step {
            id: None,
            name: "create".into(),
            description: None,
            action: Action::Fs {
                op: FsOp::Create {
                    path: vec![target.to_string_lossy().into()],
                    recurse: false,
                    content: None,
                    expand: ExpandFlags::NONE,
                },
                if_exists: None,
                if_not_exists: None,
            },
            on_success: None,
            on_failure: None,
            on_return: None,
            then: vec![],
            depends_on: vec![],
            meta: StepMeta::default(),
        };
        runner(HashMap::new()).run_step(&step, 0).unwrap();
        assert!(target.is_file());
    }

    #[test]
    fn fs_create_file_with_content() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("hello.txt");
        let step = Step {
            id: None,
            name: "create".into(),
            description: None,
            action: Action::Fs {
                op: FsOp::Create {
                    path: vec![target.to_string_lossy().into()],
                    recurse: false,
                    content: Some("hello world".into()),
                    expand: ExpandFlags::NONE,
                },
                if_exists: None,
                if_not_exists: None,
            },
            on_success: None,
            on_failure: None,
            on_return: None,
            then: vec![],
            depends_on: vec![],
            meta: StepMeta::default(),
        };
        runner(HashMap::new()).run_step(&step, 0).unwrap();
        assert_eq!(fs::read_to_string(&target).unwrap(), "hello world");
    }

    #[test]
    fn fs_create_append() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("log.txt");
        fs::write(&target, "existing\n").unwrap();
        let step = Step {
            id: None,
            name: "append".into(),
            description: None,
            action: Action::Fs {
                op: FsOp::Create {
                    path: vec![target.to_string_lossy().into()],
                    recurse: false,
                    content: Some("new line\n".into()),
                    expand: ExpandFlags::NONE,
                },
                if_exists: Some(Condition::Action(ConditionAction::Append)),
                if_not_exists: None,
            },
            on_success: None,
            on_failure: None,
            on_return: None,
            then: vec![],
            depends_on: vec![],
            meta: StepMeta::default(),
        };
        runner(HashMap::new()).run_step(&step, 0).unwrap();
        assert_eq!(fs::read_to_string(&target).unwrap(), "existing\nnew line\n");
    }

    #[test]
    fn fs_copy_append() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src.txt");
        let dst = dir.path().join("dst.txt");
        fs::write(&src, "from src\n").unwrap();
        fs::write(&dst, "original\n").unwrap();
        let step = Step {
            id: None,
            name: "append".into(),
            description: None,
            action: Action::Fs {
                op: FsOp::Copy {
                    from: src.to_string_lossy().into(),
                    to: dst.to_string_lossy().into(),
                    expand: ExpandFlags::NONE,
                },
                if_exists: Some(Condition::Action(ConditionAction::Append)),
                if_not_exists: None,
            },
            on_success: None,
            on_failure: None,
            on_return: None,
            then: vec![],
            depends_on: vec![],
            meta: StepMeta::default(),
        };
        runner(HashMap::new()).run_step(&step, 0).unwrap();
        assert_eq!(fs::read_to_string(&dst).unwrap(), "original\nfrom src\n");
    }

    #[test]
    fn fs_symlink_creates() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src.txt");
        let dst = dir.path().join("dst.txt");
        fs::write(&src, "hello").unwrap();
        let step = Step {
            id: None,
            name: "link".into(),
            description: None,
            action: Action::Fs {
                op: FsOp::Symlink {
                    from: src.to_string_lossy().into(),
                    to: dst.to_string_lossy().into(),
                    expand: ExpandFlags::NONE,
                },
                if_exists: None,
                if_not_exists: None,
            },
            on_success: None,
            on_failure: None,
            on_return: None,
            then: vec![],
            depends_on: vec![],
            meta: StepMeta::default(),
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
            id: None,
            name: "copy".into(),
            description: None,
            action: Action::Fs {
                op: FsOp::Copy {
                    from: src.to_string_lossy().into(),
                    to: dst.to_string_lossy().into(),
                    expand: ExpandFlags::NONE,
                },
                if_exists: None,
                if_not_exists: Some(Condition::Action(ConditionAction::Panic)),
            },
            on_success: None,
            on_failure: None,
            on_return: None,
            then: vec![],
            depends_on: vec![],
            meta: StepMeta::default(),
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
            id: None,
            name: "mv".into(),
            description: None,
            action: Action::Fs {
                op: FsOp::Move {
                    from: src.to_string_lossy().into(),
                    to: dst.to_string_lossy().into(),
                    expand: ExpandFlags::NONE,
                },
                if_exists: None,
                if_not_exists: None,
            },
            on_success: None,
            on_failure: None,
            on_return: None,
            then: vec![],
            depends_on: vec![],
            meta: StepMeta::default(),
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
            id: None,
            name: "del".into(),
            description: None,
            action: Action::Fs {
                op: FsOp::Delete {
                    path: vec![f.to_string_lossy().into()],
                    recurse: false,
                    expand: ExpandFlags::NONE,
                },
                if_exists: None,
                if_not_exists: None,
            },
            on_success: None,
            on_failure: None,
            on_return: None,
            then: vec![],
            depends_on: vec![],
            meta: StepMeta::default(),
        };
        runner(HashMap::new()).run_step(&step, 0).unwrap();
        assert!(!f.exists());
    }

    #[test]
    fn fs_delete_not_exists_skip() {
        let step = Step {
            id: None,
            name: "del".into(),
            description: None,
            action: Action::Fs {
                op: FsOp::Delete {
                    path: vec!["/tmp/nonexistent_rig_test".into()],
                    recurse: false,
                    expand: ExpandFlags::NONE,
                },
                if_exists: None,
                if_not_exists: Some(Condition::Action(ConditionAction::Skip)),
            },
            on_success: None,
            on_failure: None,
            on_return: None,
            then: vec![],
            depends_on: vec![],
            meta: StepMeta::default(),
        };
        runner(HashMap::new()).run_step(&step, 0).unwrap();
    }

    #[test]
    fn fs_copy_expand_false_preserves_literal_braces() {
        // A bundle-style path with a literal `{{name}}` segment. With
        // `expand: false` on both sides, the path is used byte-exact and
        // {{name}} is NOT looked up in the scope.
        let dir = tempfile::tempdir().unwrap();
        let src_dir = dir.path().join("{{name}}");
        std::fs::create_dir_all(&src_dir).unwrap();
        let src = src_dir.join("file.txt");
        std::fs::write(&src, "hi").unwrap();

        let dst_dir = dir.path().join("out");
        std::fs::create_dir_all(&dst_dir).unwrap();
        let dst = dst_dir.join("{{name}}.txt");

        let step = Step {
            id: None,
            name: "copy-literal".into(),
            description: None,
            action: Action::Fs {
                op: FsOp::Copy {
                    from: src.to_string_lossy().into(),
                    to: dst.to_string_lossy().into(),
                    expand: ExpandFlags::NONE,
                },
                if_exists: None,
                if_not_exists: None,
            },
            on_success: None,
            on_failure: None,
            on_return: None,
            then: vec![],
            depends_on: vec![],
            meta: StepMeta::default(),
        };

        // Set a `name` var so we'd notice if substitution leaked.
        let mut scope = crate::vars::Scope::default();
        scope.set("name", "WRONG".into());
        let r = Runner::new(HashMap::new(), false, false, Meta::default(), scope);
        r.run_step(&step, 0).unwrap();

        // The literal destination with `{{name}}.txt` should exist.
        assert!(dst.is_file());
        assert_eq!(std::fs::read_to_string(&dst).unwrap(), "hi");
        // And nothing under `out/WRONG.txt` should have been created.
        assert!(!dst_dir.join("WRONG.txt").exists());
    }

    #[test]
    fn fs_copy_default_expands_path_vars() {
        // Default behavior (ExpandFlags::PATHS) keeps legacy substitution on
        // path fields, so {{name}} gets rendered.
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src.txt");
        std::fs::write(&src, "payload").unwrap();
        let dst_template = format!("{}/{{{{name}}}}.txt", dir.path().display());

        let step = Step {
            id: None,
            name: "copy-default".into(),
            description: None,
            action: Action::Fs {
                op: FsOp::Copy {
                    from: src.to_string_lossy().into(),
                    to: dst_template,
                    expand: ExpandFlags::default(),
                },
                if_exists: None,
                if_not_exists: None,
            },
            on_success: None,
            on_failure: None,
            on_return: None,
            then: vec![],
            depends_on: vec![],
            meta: StepMeta::default(),
        };

        let mut scope = crate::vars::Scope::default();
        scope.set("name", "resolved".into());
        let r = Runner::new(HashMap::new(), false, false, Meta::default(), scope);
        r.run_step(&step, 0).unwrap();

        assert!(dir.path().join("resolved.txt").is_file());
    }

    /// Construct a BundleCtx rooted at `dir` with the given binary globs.
    fn bundle_ctx_at(dir: &std::path::Path, binary: &[&str]) -> crate::bundle::BundleCtx {
        let patterns: Vec<String> = binary.iter().map(|s| s.to_string()).collect();
        crate::bundle::BundleCtx {
            root: dir.to_path_buf(),
            binary: crate::bundle::BinaryMatcher::new(&patterns).unwrap(),
            cleanup: crate::bundle::Cleanup::Never,
            succeeded: std::sync::atomic::AtomicBool::new(false),
            _temp_dir: None,
        }
    }

    #[test]
    fn fs_copy_in_bundle_renders_templated_contents() {
        let bundle_dir = tempfile::tempdir().unwrap();
        // source lives inside the bundle root; contents contain {{name}}
        let src = bundle_dir.path().join("greet.txt");
        std::fs::write(&src, "hello {{name}}\n").unwrap();
        let out = tempfile::tempdir().unwrap();
        let dst = out.path().join("out.txt");

        let step = Step {
            id: None,
            name: "copy".into(),
            description: None,
            action: Action::Fs {
                op: FsOp::Copy {
                    from: src.to_string_lossy().into(),
                    to: dst.to_string_lossy().into(),
                    expand: ExpandFlags::default(), // contents:false, yet bundle rule overrides
                },
                if_exists: None,
                if_not_exists: None,
            },
            on_success: None,
            on_failure: None,
            on_return: None,
            then: vec![],
            depends_on: vec![],
            meta: StepMeta::default(),
        };

        let mut scope = crate::vars::Scope::default();
        scope.set("name", "bundle".into());
        let r = Runner::new_with_bundle(
            HashMap::new(),
            false,
            false,
            Meta::default(),
            scope,
            bundle_ctx_at(bundle_dir.path(), &[]),
        );
        r.run_step(&step, 0).unwrap();
        assert_eq!(std::fs::read_to_string(&dst).unwrap(), "hello bundle\n");
    }

    #[test]
    fn fs_copy_in_bundle_binary_glob_stays_byte_exact() {
        let bundle_dir = tempfile::tempdir().unwrap();
        let src = bundle_dir.path().join("logo.png"); // matches *.png
        std::fs::write(&src, b"PNG {{name}} bytes").unwrap();
        let out = tempfile::tempdir().unwrap();
        let dst = out.path().join("out.png");

        let step = Step {
            id: None,
            name: "copy".into(),
            description: None,
            action: Action::Fs {
                op: FsOp::Copy {
                    from: src.to_string_lossy().into(),
                    to: dst.to_string_lossy().into(),
                    expand: ExpandFlags::default(),
                },
                if_exists: None,
                if_not_exists: None,
            },
            on_success: None,
            on_failure: None,
            on_return: None,
            then: vec![],
            depends_on: vec![],
            meta: StepMeta::default(),
        };

        let mut scope = crate::vars::Scope::default();
        scope.set("name", "SHOULD-NOT-APPEAR".into());
        let r = Runner::new_with_bundle(
            HashMap::new(),
            false,
            false,
            Meta::default(),
            scope,
            bundle_ctx_at(bundle_dir.path(), &["*.png"]),
        );
        r.run_step(&step, 0).unwrap();
        assert_eq!(std::fs::read(&dst).unwrap(), b"PNG {{name}} bytes");
    }

    #[test]
    fn fs_copy_outside_bundle_root_is_byte_exact() {
        // Bundle is rooted elsewhere; source file lives outside it — the
        // bundle-rendering rule must not touch a template here.
        let bundle_dir = tempfile::tempdir().unwrap();
        let workspace = tempfile::tempdir().unwrap();
        let src = workspace.path().join("raw.txt"); // outside bundle root
        std::fs::write(&src, "hi {{name}}").unwrap();
        let dst = workspace.path().join("out.txt");

        let step = Step {
            id: None,
            name: "copy".into(),
            description: None,
            action: Action::Fs {
                op: FsOp::Copy {
                    from: src.to_string_lossy().into(),
                    to: dst.to_string_lossy().into(),
                    expand: ExpandFlags::default(),
                },
                if_exists: None,
                if_not_exists: None,
            },
            on_success: None,
            on_failure: None,
            on_return: None,
            then: vec![],
            depends_on: vec![],
            meta: StepMeta::default(),
        };

        let mut scope = crate::vars::Scope::default();
        scope.set("name", "world".into());
        let r = Runner::new_with_bundle(
            HashMap::new(),
            false,
            false,
            Meta::default(),
            scope,
            bundle_ctx_at(bundle_dir.path(), &[]),
        );
        r.run_step(&step, 0).unwrap();
        assert_eq!(std::fs::read_to_string(&dst).unwrap(), "hi {{name}}");
    }

    #[test]
    fn fs_copy_outside_bundle_no_ctx_is_byte_exact() {
        // No bundle context at all — matches today's legacy behavior.
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("src.txt");
        std::fs::write(&src, "{{name}} template").unwrap();
        let dst = dir.path().join("out.txt");

        let step = Step {
            id: None,
            name: "copy".into(),
            description: None,
            action: Action::Fs {
                op: FsOp::Copy {
                    from: src.to_string_lossy().into(),
                    to: dst.to_string_lossy().into(),
                    expand: ExpandFlags::default(),
                },
                if_exists: None,
                if_not_exists: None,
            },
            on_success: None,
            on_failure: None,
            on_return: None,
            then: vec![],
            depends_on: vec![],
            meta: StepMeta::default(),
        };

        let mut scope = crate::vars::Scope::default();
        scope.set("name", "x".into());
        let r = Runner::new(HashMap::new(), false, false, Meta::default(), scope);
        r.run_step(&step, 0).unwrap();
        assert_eq!(std::fs::read_to_string(&dst).unwrap(), "{{name}} template");
    }

    #[test]
    fn then_runs_after_parent() {
        let dir = tempfile::tempdir().unwrap();
        let step = Step {
            id: None,
            name: "parent".into(),
            description: None,
            action: Action::Shell {
                commands: vec!["echo parent".into()],
                dir: None,
                env: None,
            },
            on_success: None,
            on_failure: None,
            on_return: None,
            then: vec![StepRef::Inline(Box::new(shell_step(
                vec!["echo child > child.txt"],
                Some(dir.path().to_str().unwrap()),
            )))],
            depends_on: vec![],
            meta: StepMeta::default(),
        };
        runner(HashMap::new()).run_step(&step, 0).unwrap();
        assert!(dir.path().join("child.txt").exists());
    }

    #[test]
    fn then_skipped_on_fallible_failure() {
        let dir = tempfile::tempdir().unwrap();
        let step = Step {
            id: None,
            name: "parent".into(),
            description: None,
            action: Action::Shell {
                commands: vec!["false".into()],
                dir: None,
                env: None,
            },
            on_success: None,
            on_failure: None,
            on_return: None,
            then: vec![StepRef::Inline(Box::new(shell_step(
                vec!["echo child > child.txt"],
                Some(dir.path().to_str().unwrap()),
            )))],
            depends_on: vec![],
            meta: StepMeta {
                fallible: true,
                ..StepMeta::default()
            },
        };
        runner(HashMap::new()).run_step(&step, 0).unwrap();
        assert!(!dir.path().join("child.txt").exists());
    }

    #[test]
    fn optional_steps_skipped() {
        let steps = vec![Step {
            id: None,
            name: "opt".into(),
            description: None,
            action: Action::Shell {
                commands: vec!["false".into()],
                dir: None,
                env: None,
            },
            on_success: None,
            on_failure: None,
            on_return: None,
            then: vec![],
            depends_on: vec![],
            meta: StepMeta {
                optional: true,
                ..StepMeta::default()
            },
        }];
        runner(HashMap::new()).run_steps(&steps).unwrap();
    }

    #[test]
    fn on_success_triggers() {
        let dir = tempfile::tempdir().unwrap();
        let handler = Step {
            id: Some("handler".into()),
            name: "handler".into(),
            description: None,
            action: Action::Shell {
                commands: vec![format!(
                    "echo handled > {}/handled.txt",
                    dir.path().display()
                )],
                dir: None,
                env: None,
            },
            on_success: None,
            on_failure: None,
            on_return: None,
            then: vec![],
            depends_on: vec![],
            meta: StepMeta {
                optional: true,
                ..StepMeta::default()
            },
        };
        let mut index = HashMap::new();
        index.insert("handler".into(), handler);

        let step = Step {
            id: None,
            name: "test".into(),
            description: None,
            action: Action::Shell {
                commands: vec!["true".into()],
                dir: None,
                env: None,
            },
            on_success: Some(vec![StepRef::Id("handler".into())]),
            on_failure: None,
            on_return: None,
            then: vec![],
            depends_on: vec![],
            meta: StepMeta::default(),
        };
        runner(index).run_step(&step, 0).unwrap();
        assert!(dir.path().join("handled.txt").exists());
    }

    #[test]
    fn on_failure_triggers() {
        let dir = tempfile::tempdir().unwrap();
        let handler = Step {
            id: Some("handler".into()),
            name: "handler".into(),
            description: None,
            action: Action::Shell {
                commands: vec![format!(
                    "echo handled > {}/handled.txt",
                    dir.path().display()
                )],
                dir: None,
                env: None,
            },
            on_success: None,
            on_failure: None,
            on_return: None,
            then: vec![],
            depends_on: vec![],
            meta: StepMeta {
                optional: true,
                ..StepMeta::default()
            },
        };
        let mut index = HashMap::new();
        index.insert("handler".into(), handler);

        let step = Step {
            id: None,
            name: "test".into(),
            description: None,
            action: Action::Shell {
                commands: vec!["false".into()],
                dir: None,
                env: None,
            },
            on_success: None,
            on_failure: Some(vec![StepRef::Id("handler".into())]),
            on_return: None,
            then: vec![],
            depends_on: vec![],
            meta: StepMeta::default(),
        };
        runner(index).run_step(&step, 0).unwrap();
        assert!(dir.path().join("handled.txt").exists());
    }

    #[test]
    fn on_return_overrides_on_success() {
        let dir = tempfile::tempdir().unwrap();
        let special = Step {
            id: Some("special".into()),
            name: "special".into(),
            description: None,
            action: Action::Shell {
                commands: vec![format!(
                    "echo special > {}/special.txt",
                    dir.path().display()
                )],
                dir: None,
                env: None,
            },
            on_success: None,
            on_failure: None,
            on_return: None,
            then: vec![],
            depends_on: vec![],
            meta: StepMeta {
                optional: true,
                ..StepMeta::default()
            },
        };
        let generic = Step {
            id: Some("generic".into()),
            name: "generic".into(),
            description: None,
            action: Action::Shell {
                commands: vec![format!(
                    "echo generic > {}/generic.txt",
                    dir.path().display()
                )],
                dir: None,
                env: None,
            },
            on_success: None,
            on_failure: None,
            on_return: None,
            then: vec![],
            depends_on: vec![],
            meta: StepMeta {
                optional: true,
                ..StepMeta::default()
            },
        };
        let mut index = HashMap::new();
        index.insert("special".into(), special);
        index.insert("generic".into(), generic);

        let step = Step {
            id: None,
            name: "test".into(),
            description: None,
            action: Action::Shell {
                commands: vec!["true".into()],
                dir: None,
                env: None,
            },
            on_success: Some(vec![StepRef::Id("generic".into())]),
            on_failure: None,
            on_return: Some(HashMap::from([(
                "0".into(),
                vec![StepRef::Id("special".into())],
            )])),
            then: vec![],
            depends_on: vec![],
            meta: StepMeta::default(),
        };
        runner(index).run_step(&step, 0).unwrap();
        assert!(dir.path().join("special.txt").exists());
        assert!(!dir.path().join("generic.txt").exists());
    }

    #[test]
    fn on_return_wildcard_overrides_on_failure() {
        let dir = tempfile::tempdir().unwrap();
        let handler = Step {
            id: Some("catch".into()),
            name: "catch".into(),
            description: None,
            action: Action::Shell {
                commands: vec![format!("echo caught > {}/caught.txt", dir.path().display())],
                dir: None,
                env: None,
            },
            on_success: None,
            on_failure: None,
            on_return: None,
            then: vec![],
            depends_on: vec![],
            meta: StepMeta {
                optional: true,
                ..StepMeta::default()
            },
        };
        let mut index = HashMap::new();
        index.insert("catch".into(), handler);

        let step = Step {
            id: None,
            name: "test".into(),
            description: None,
            action: Action::Shell {
                commands: vec!["exit 42".into()],
                dir: None,
                env: None,
            },
            on_success: None,
            on_failure: Some(vec![StepRef::Id("catch".into())]),
            on_return: Some(HashMap::from([(
                "_".into(),
                vec![StepRef::Id("catch".into())],
            )])),
            then: vec![],
            depends_on: vec![],
            meta: StepMeta::default(),
        };
        runner(index).run_step(&step, 0).unwrap();
        assert!(dir.path().join("caught.txt").exists());
    }

    #[test]
    fn on_success_then_both_run() {
        let dir = tempfile::tempdir().unwrap();
        let handler = Step {
            id: Some("handler".into()),
            name: "handler".into(),
            description: None,
            action: Action::Shell {
                commands: vec![format!(
                    "echo handled > {}/handled.txt",
                    dir.path().display()
                )],
                dir: None,
                env: None,
            },
            on_success: None,
            on_failure: None,
            on_return: None,
            then: vec![],
            depends_on: vec![],
            meta: StepMeta {
                optional: true,
                ..StepMeta::default()
            },
        };
        let mut index = HashMap::new();
        index.insert("handler".into(), handler);

        let step = Step {
            id: None,
            name: "test".into(),
            description: None,
            action: Action::Shell {
                commands: vec!["true".into()],
                dir: None,
                env: None,
            },
            on_success: Some(vec![StepRef::Id("handler".into())]),
            on_failure: None,
            on_return: None,
            then: vec![StepRef::Inline(Box::new(shell_step(
                vec!["echo child > child.txt"],
                Some(dir.path().to_str().unwrap()),
            )))],
            depends_on: vec![],
            meta: StepMeta::default(),
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
            id: None,
            name: "retry-test".into(),
            description: None,
            action: Action::Shell {
                commands: vec![format!("echo x >> {} && false", counter.display())],
                dir: None,
                env: None,
            },
            on_success: None,
            on_failure: None,
            on_return: None,
            then: vec![],
            depends_on: vec![],
            meta: StepMeta {
                retries: Some(2),
                ..StepMeta::default()
            },
        };
        let result = runner(HashMap::new()).run_step(&step, 0);
        assert!(result.is_err());
        let content = fs::read_to_string(&counter).unwrap();
        assert_eq!(content.lines().count(), 3); // 1 + 2 retries
    }

    #[test]
    fn cycle_detected() {
        let step = Step {
            id: Some("a".into()),
            name: "a".into(),
            description: None,
            action: Action::Shell {
                commands: vec!["true".into()],
                dir: None,
                env: None,
            },
            on_success: None,
            on_failure: None,
            on_return: None,
            then: vec![StepRef::Id("a".into())],
            depends_on: vec![],
            meta: StepMeta::default(),
        };
        let mut index = HashMap::new();
        index.insert("a".into(), step.clone());
        let r = Runner::new(
            index,
            false,
            false,
            Meta::default(),
            crate::vars::Scope::default(),
        );
        let result = r.run_step(&step, 0);
        assert!(result.is_err());
    }

    #[test]
    fn rig_action_runs_sub_config() {
        let td = tempfile::tempdir().unwrap();
        let sub_path = td.path().join("sub.json");
        std::fs::write(&sub_path, r#"{"name":"sub","version":"1.0.0","meta":{"vars":{"msg":"default"}},"steps":[{"name":"echo","action":{"kind":"shell","commands":["echo {{msg}}"]}}]}"#).unwrap();

        let step = Step {
            id: None,
            name: "run-sub".into(),
            description: None,
            action: Action::Rig {
                file: sub_path.to_str().unwrap().to_string(),
                set: Some(
                    [("msg".to_string(), "hello".to_string())]
                        .into_iter()
                        .collect(),
                ),
            },
            on_success: None,
            on_failure: None,
            on_return: None,
            then: vec![],
            depends_on: vec![],
            meta: StepMeta::default(),
        };
        let index = HashMap::new();
        let r = Runner::new(
            index,
            false,
            false,
            Meta::default(),
            crate::vars::Scope::default(),
        );
        let result = r.run_step(&step, 0);
        assert!(result.is_ok());
    }
}
