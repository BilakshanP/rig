mod config;
mod executor;
mod path;
mod style;

use clap::Parser;
use config::*;
use std::collections::{HashMap, HashSet};
use std::process::ExitCode;

#[derive(Parser)]
#[command(name = "rig", about = "Bootstrap dev environments from a JSON config")]
struct Cli {
    /// Path or URL to the JSON config file
    config: String,
    /// Print what would happen without executing
    #[arg(long)]
    dry_run: bool,
    /// Show suppressed output (meta.silent)
    #[arg(long, short)]
    verbose: bool,
    /// Run only the step with this ID
    #[arg(long)]
    only: Option<String>,
    /// Parse and validate config without executing
    #[arg(long)]
    validate: bool,
    /// List all steps (one line each)
    #[arg(long)]
    list: bool,
    /// Describe a step by ID
    #[arg(long)]
    describe: Option<String>,
    /// Expand then/handler sub-steps (optional depth limit, default: unlimited)
    #[arg(long, num_args = 0..=1, default_missing_value = "0")]
    depth: Option<u32>,
    /// Set a variable: --set key=value (repeatable, used as {{key}} in config)
    #[arg(long = "set", value_name = "KEY=VALUE")]
    vars: Vec<String>,
}

fn fetch_url(url: &str) -> Result<std::path::PathBuf, String> {
    let body = ureq::get(url)
        .call()
        .map_err(|e| format!("failed to fetch {url}: {e}"))?
        .body_mut()
        .read_to_string()
        .map_err(|e| format!("failed to read response: {e}"))?;
    let tmp = std::env::temp_dir().join("rig-remote-config.jsonc");
    std::fs::write(&tmp, body.as_bytes())
        .map_err(|e| format!("failed to write temp file: {e}"))?;
    Ok(tmp)
}

// -- --list: one-line-per-step summary --

fn print_list(steps: &[Step], verbose: bool) {
    // Find max widths for alignment
    let id_width = steps.iter()
        .map(|s| s.id.as_deref().unwrap_or("-").len())
        .max().unwrap_or(0);

    let name_width = steps.iter()
        .map(|s| s.name.len())
        .max().unwrap_or(0);

    for step in steps {
        let id = step.id.as_deref().unwrap_or("-");
        let mut flags = Vec::new();
        if step.meta.optional { flags.push("optional".to_string()); }
        if step.meta.fallible { flags.push("fallible".to_string()); }
        if step.meta.sudo { flags.push("sudo".to_string()); }
        if !step.meta.silent.is_empty() {
            let s: Vec<_> = step.meta.silent.iter().map(|s| format!("{s:?}").to_lowercase()).collect();
            flags.push(format!("silent: {}", s.join(", ")));
        }
        if let Some(r) = step.meta.retries { flags.push(format!("retries: {r}")); }

        let flag_str = if flags.is_empty() {
            String::new()
        } else {
            format!("  {}", style::render(&format!("<fy>[{}]</f>", flags.join("] ["))))
        };

        let desc = if verbose {
            step.description.as_deref().map(|d| format!("  {}", style::render(&format!("<md>{d}</m>")))).unwrap_or_default()
        } else {
            String::new()
        };

        println!("{}{flag_str}{desc}", style::render(&format!(
            "<fc>{id:<id_width$}</f>  <mb>{:<name_width$}</m>",
            step.name
        )));
    }
}

// -- --describe: detailed step view --

fn describe_step(step: &Step, index: &HashMap<String, Step>, max_depth: Option<u32>, verbose: bool) {
    let mut seen = HashSet::new();
    describe_step_inner(step, index, 0, max_depth, verbose, &mut seen);
}

fn describe_step_inner(
    step: &Step,
    index: &HashMap<String, Step>,
    depth: u32,
    max_depth: Option<u32>,
    verbose: bool,
    seen: &mut HashSet<String>,
) {
    let indent = "  ".repeat(depth as usize);

    // Mark as seen for loop protection
    if let Some(id) = &step.id
        && !seen.insert(id.clone())
    {
        println!("{indent}{}", style::render(&format!("<md>(cycle: {id} already shown)</m>")));
        return;
    }

    // Header
    let id_str = step.id.as_deref().map(|id| format!("{id}: ")).unwrap_or_default();
    println!("{indent}{}", style::render(&format!("<fc>{id_str}</f><mb>{}</m>", step.name)));

    if verbose
        && let Some(desc) = &step.description
    {
        println!("{indent}  {}", style::render(&format!("<md>{desc}</m>")));
    }

    let ai = format!("{indent}  ");

    // Action
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
            println!("{ai}{}", style::render(&format!("<md>git clone</m> {repo} -> {dest}")));
            if *on_conflict != GitOnConflict::Skip {
                println!("{ai}{}", style::render(&format!("<md>on-conflict:</m> {on_conflict:?}")));
            }
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
                FsOp::Symlink { from, to } => println!("{ai}{}", style::render(&format!("<md>symlink</m> {from} -> {to}"))),
                FsOp::Copy { from, to } => println!("{ai}{}", style::render(&format!("<md>copy</m> {from} -> {to}"))),
                FsOp::Move { from, to } => println!("{ai}{}", style::render(&format!("<md>move</m> {from} -> {to}"))),
                FsOp::Delete { path, recurse } => {
                    for p in path_list(path) { println!("{ai}{}", style::render(&format!("<md>delete:</m> {p}"))); }
                    if *recurse { println!("{ai}{}", style::render("<md>recurse:</m> true")); }
                }
            }
            if let Some(c) = if_exists { println!("{ai}{}", style::render(&format!("<md>if-exists:</m> {}", condition_label(c)))); }
            if let Some(c) = if_not_exists { println!("{ai}{}", style::render(&format!("<md>if-not-exists:</m> {}", condition_label(c)))); }
        }
        Action::Io { level, message, markup } => {
            let ml = if *markup { " [markup]" } else { "" };
            println!("{ai}{}", style::render(&format!("<md>{level:?}:</m> {message:?}{ml}")));
        }
    }

    // Handlers
    if let Some(refs) = &step.on_success { println!("{ai}{}", style::render(&format!("<md>on-success:</m> {}", step_refs_label(refs)))); }
    if let Some(refs) = &step.on_failure { println!("{ai}{}", style::render(&format!("<md>on-failure:</m> {}", step_refs_label(refs)))); }
    if let Some(map) = &step.on_return {
        println!("{ai}{}", style::render("<md>on-return:</m>"));
        for (code, refs) in map { println!("{ai}  {}", style::render(&format!("{code} -> <fc>{}</f>", step_refs_label(refs)))); }
    }

    // Meta flags
    let mut flags = Vec::new();
    if step.meta.optional { flags.push("optional"); }
    if step.meta.fallible { flags.push("fallible"); }
    if step.meta.sudo { flags.push("sudo"); }
    if !flags.is_empty() {
        println!("{ai}{}", style::render(&format!("<md>meta:</m> {}", flags.join(", "))));
    }

    // Then (only expand if depth allows)
    if !step.then.is_empty() {
        let should_expand = max_depth.is_some();
        let can_go_deeper = max_depth.is_none_or(|m| m == 0 || depth + 1 < m);

        if should_expand && can_go_deeper {
            println!("{ai}{}", style::render("<md>then:</m>"));
            for child in &step.then {
                match child {
                    ChildRef::Id(id) => {
                        if let Some(s) = index.get(id) {
                            describe_step_inner(s, index, depth + 1, max_depth, verbose, seen);
                        } else {
                            println!("{ai}  -> {id}");
                        }
                    }
                    ChildRef::Inline(s) => describe_step_inner(s, index, depth + 1, max_depth, verbose, seen),
                }
            }
        } else {
            let refs: Vec<_> = step.then.iter().map(|c| match c {
                ChildRef::Id(id) => id.clone(),
                ChildRef::Inline(s) => format!("[inline: {}]", s.name),
            }).collect();
            println!("{ai}{}", style::render(&format!("<md>then:</m> {}", refs.join(", "))));
        }
    }
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

fn main() -> ExitCode {
    let cli = Cli::parse();

    let vars: HashMap<String, String> = cli.vars.iter().filter_map(|s| {
        let (k, v) = s.split_once('=')?;
        Some((k.to_string(), v.to_string()))
    }).collect();

    let (config_path, _tmp) = if cli.config.starts_with("http://") || cli.config.starts_with("https://") {
        match fetch_url(&cli.config) {
            Ok(p) => {
                let s = p.to_string_lossy().into_owned();
                (s, Some(p))
            }
            Err(e) => {
                eprintln!("{}", style::render(&format!("<fr>error:</f> {e}")));
                return ExitCode::FAILURE;
            }
        }
    } else {
        (cli.config.clone(), None)
    };

    let cfg = match config::parse_config(&config_path, &vars) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{}", style::render(&format!("<fr>error:</f> {e}")));
            return ExitCode::FAILURE;
        }
    };

    if cli.validate {
        println!("{}", style::render(&format!("<fg>ok:</f> config valid: <mb>{}</m> ({} steps)", cfg.name, cfg.steps.len())));
        return ExitCode::SUCCESS;
    }

    let index = config::build_step_index(&cfg);

    if cli.list {
        print_list(&cfg.steps, cli.verbose);
        return ExitCode::SUCCESS;
    }

    if let Some(id) = &cli.describe {
        match index.get(id) {
            Some(step) => {
                describe_step(step, &index, cli.depth, cli.verbose);
                return ExitCode::SUCCESS;
            }
            None => {
                eprintln!("{}", style::render(&format!("<fr>error:</f> no step with id '<fc>{id}</f>'")));
                return ExitCode::FAILURE;
            }
        }
    }

    let runner = executor::Runner::new(index, cli.dry_run, cli.verbose, cfg.meta.clone());
    let cwd = std::env::current_dir().map(|p| p.display().to_string()).unwrap_or_default();

    if cli.dry_run {
        println!("{}", style::render(&format!("<fc>[dry-run]</f> <mb>{}</m> <md>({cwd})</m>", cfg.name)));
        if let Some(id) = &cli.only {
            match runner.index.get(id) {
                Some(step) => runner.dry_run_audit(&cfg, std::slice::from_ref(step)),
                None => {
                    eprintln!("{}", style::render(&format!("<fr>error:</f> no step with id '<fc>{id}</f>'")));
                    return ExitCode::FAILURE;
                }
            }
        } else {
            runner.dry_run_audit(&cfg, &cfg.steps);
        }
        return ExitCode::SUCCESS;
    }

    println!("{}", style::render(&format!("<fg>Running:</f> <mb>{}</m> <md>({cwd})</m>", cfg.name)));

    if let Some(id) = &cli.only {
        match runner.index.get(id) {
            Some(step) => {
                if let Err(e) = runner.run_step(step, 0) {
                    eprintln!("{}", style::render(&format!("<fr>error in step '{id}':</f> {e}")));
                    return ExitCode::FAILURE;
                }
            }
            None => {
                eprintln!("{}", style::render(&format!("<fr>error:</f> no step with id '<fc>{id}</f>'")));
                return ExitCode::FAILURE;
            }
        }
    } else if let Err(e) = runner.run_steps(&cfg.steps) {
        eprintln!("{}", style::render(&format!("<fr>error:</f> {e}")));
        return ExitCode::FAILURE;
    }

    println!("{}", style::render("<fg>Done.</f>"));
    ExitCode::SUCCESS
}
