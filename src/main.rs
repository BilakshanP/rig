mod config;
mod executor;
mod path;

use clap::Parser;
use std::path::PathBuf;
use std::process::ExitCode;

#[derive(Parser)]
#[command(name = "devsetup", about = "Bootstrap dev environments from a JSON config")]
struct Cli {
    /// Path to the JSON config file
    config: PathBuf,
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
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let cfg = match config::parse_config(cli.config.to_str().unwrap_or_default()) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::FAILURE;
        }
    };

    if cli.validate {
        println!("✓ config valid: {} ({} steps)", cfg.name, cfg.steps.len());
        return ExitCode::SUCCESS;
    }

    let runner = executor::Runner::new(
        config::build_step_index(&cfg),
        cli.dry_run,
        cli.verbose,
        cfg.max_retries,
    );

    if cli.dry_run {
        println!("[dry-run] {}", cfg.name);
        if let Some(id) = &cli.only {
            match runner.index.get(id) {
                Some(step) => runner.dry_run_audit(std::slice::from_ref(step)),
                None => {
                    eprintln!("error: no step with id '{id}'");
                    return ExitCode::FAILURE;
                }
            }
        } else {
            runner.dry_run_audit(&cfg.steps);
        }
        return ExitCode::SUCCESS;
    }

    println!("Running: {}", cfg.name);

    if let Some(id) = &cli.only {
        match runner.index.get(id) {
            Some(step) => {
                if let Err(e) = runner.run_step(step, 0) {
                    eprintln!("error in step '{id}': {e}");
                    return ExitCode::FAILURE;
                }
            }
            None => {
                eprintln!("error: no step with id '{id}'");
                return ExitCode::FAILURE;
            }
        }
    } else if let Err(e) = runner.run_steps(&cfg.steps) {
        eprintln!("error: {e}");
        return ExitCode::FAILURE;
    }

    println!("Done.");
    ExitCode::SUCCESS
}
