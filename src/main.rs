mod config;
mod executor;
mod inspect;
mod path;
mod style;

use clap::Parser;
use std::collections::HashMap;
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
    /// Keep undefined variables as {{var}} instead of failing
    #[arg(long)]
    placeholder: bool,
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

    let cfg = match config::parse_config(&config_path, &vars, cli.placeholder) {
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
        inspect::print_list(&cfg.steps, cli.verbose);
        return ExitCode::SUCCESS;
    }

    if let Some(id) = &cli.describe {
        match index.get(id) {
            Some(step) => {
                inspect::describe_step(step, &index, cli.depth, cli.verbose);
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
