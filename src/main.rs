mod bundle;
mod config;
mod executor;
mod inspect;
mod path;
mod style;
mod vars;

use clap::{Parser, Subcommand};
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::ExitCode;

#[derive(Parser)]
#[command(
    name = "rig",
    about = "A powerful, cross-platform CLI tool for automating structured workflows from declarative JSON configs"
)]
struct Cli {
    /// Subcommand (omit to run a config/bundle)
    #[command(subcommand)]
    command: Option<Commands>,

    /// Path or URL to the JSON config or .rig bundle (required unless a subcommand is given)
    config: Option<String>,

    /// Print what would happen without executing
    #[arg(long)]
    dry_run: bool,
    /// Show suppressed output (meta.silent)
    #[arg(long, short)]
    verbose: bool,
    /// Suppress output (-q: chrome, -qq: all output, -qqq: even errors)
    #[arg(short = 'q', long, action = clap::ArgAction::Count)]
    quiet: u8,
    /// Suppress command stdout/stderr (show only rig chrome)
    #[arg(long, short = 's')]
    silent: bool,
    /// Run steps in parallel when depends-on allows it
    #[arg(long)]
    parallel: bool,
    /// Force sequential execution (overrides meta.parallel)
    #[arg(long)]
    no_parallel: bool,
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
    /// List all variables referenced in config with their defaults
    #[arg(long = "vars")]
    list_vars: bool,
    /// Show execution graph (combine with --dot for Graphviz DOT output)
    #[arg(long)]
    graph: bool,
    /// Output in DOT format (use with --graph)
    #[arg(long)]
    dot: bool,
    /// Filter edge types in graph (comma-separated: seq,depends-on,then,on-success,on-failure,on-return)
    #[arg(long, value_delimiter = ',')]
    edges: Vec<String>,
    /// Show edge labels in graph output
    #[arg(long)]
    label: bool,
    /// Set a variable: --set key=value (repeatable, used as {{key}} in config)
    #[arg(long = "set", value_name = "KEY=VALUE")]
    set_vars: Vec<String>,
}

#[derive(Subcommand)]
enum Commands {
    /// Pack a directory into a .rig bundle (tar.gz)
    Pack {
        /// Source directory (must contain manifest.json or manifest.jsonc at the root)
        src: PathBuf,
        /// Output path. Defaults to `<basename(src)>.rig` in the current directory.
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
    /// Unpack a .rig bundle into a directory
    Unpack {
        /// Bundle archive
        archive: PathBuf,
        /// Destination directory. Defaults to the archive filename with
        /// `.rig` stripped (must be a `.rig` filename when omitted).
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
    /// Show manifest metadata and file list of a .rig bundle
    Info {
        /// Bundle archive
        archive: PathBuf,
    },
}

fn fetch_url(url: &str) -> Result<std::path::PathBuf, String> {
    // Preserve the `.rig` extension when the URL points at a bundle so the
    // bundle detection path later on picks it up without sniffing.
    let tmp_name = if url.ends_with(".rig") {
        "rig-remote-bundle.rig"
    } else {
        "rig-remote-config.jsonc"
    };
    let tmp = std::env::temp_dir().join(tmp_name);

    if url.ends_with(".rig") {
        // Binary-safe path for archives.
        let mut resp = ureq::get(url)
            .call()
            .map_err(|e| format!("failed to fetch {url}: {e}"))?;
        let mut reader = resp.body_mut().as_reader();
        let mut buf = Vec::new();
        std::io::Read::read_to_end(&mut reader, &mut buf)
            .map_err(|e| format!("failed to read response: {e}"))?;
        std::fs::write(&tmp, &buf).map_err(|e| format!("failed to write temp file: {e}"))?;
    } else {
        let body = ureq::get(url)
            .call()
            .map_err(|e| format!("failed to fetch {url}: {e}"))?
            .body_mut()
            .read_to_string()
            .map_err(|e| format!("failed to read response: {e}"))?;
        std::fs::write(&tmp, body.as_bytes())
            .map_err(|e| format!("failed to write temp file: {e}"))?;
    }
    Ok(tmp)
}

/// Resolve the raw input (path, URL, git repo, directory, or .rig bundle) into
/// a config file path and an optional BundleCtx.
fn resolve_input(
    raw: &str,
    vars: &HashMap<String, String>,
    placeholder: bool,
) -> Result<(String, Option<bundle::BundleCtx>), String> {
    let is_url = raw.starts_with("http://") || raw.starts_with("https://");
    let is_git_ssh = raw.starts_with("git@") || raw.starts_with("ssh://");
    let is_local_dir = !is_url && !is_git_ssh && std::path::Path::new(raw).is_dir();

    if is_local_dir {
        let (_cfg, ctx) = bundle::open_directory(std::path::Path::new(raw), vars, placeholder)
            .map_err(|e| e.to_string())?;
        Ok(manifest_from_ctx(ctx))
    } else if is_git_ssh || (is_url && bundle::looks_like_git_repo(raw)) {
        let (_cfg, ctx) =
            bundle::open_git_repo(raw, vars, placeholder).map_err(|e| e.to_string())?;
        Ok(manifest_from_ctx(ctx))
    } else if is_url {
        let p = fetch_url(raw)?;
        if bundle::looks_like_bundle(&p) {
            let (_cfg, ctx) =
                bundle::open_bundle(&p, vars, placeholder).map_err(|e| e.to_string())?;
            Ok(manifest_from_ctx(ctx))
        } else {
            Ok((p.to_string_lossy().into_owned(), None))
        }
    } else if bundle::looks_like_bundle(std::path::Path::new(raw)) {
        let (_cfg, ctx) = bundle::open_bundle(std::path::Path::new(raw), vars, placeholder)
            .map_err(|e| e.to_string())?;
        Ok(manifest_from_ctx(ctx))
    } else {
        Ok((raw.to_string(), None))
    }
}

/// Extract the manifest path from a BundleCtx (prefers .jsonc over .json).
fn manifest_from_ctx(ctx: bundle::BundleCtx) -> (String, Option<bundle::BundleCtx>) {
    let manifest = if ctx.root.join("manifest.jsonc").is_file() {
        ctx.root.join("manifest.jsonc")
    } else {
        ctx.root.join("manifest.json")
    };
    (manifest.to_string_lossy().into_owned(), Some(ctx))
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    // Dispatch subcommands first; they don't use the main config flow.
    if let Some(cmd) = cli.command {
        return run_subcommand(cmd);
    }

    let Some(raw_config) = cli.config else {
        eprintln!(
            "{}",
            style::render(
                "<s fR mb>error:</s> no config/bundle given\n\nFor more information, try '<mb>--help</m>'."
            )
        );
        return ExitCode::FAILURE;
    };

    let vars: HashMap<String, String> = cli
        .set_vars
        .iter()
        .filter_map(|s| {
            let (k, v) = s.split_once('=')?;
            Some((k.to_string(), v.to_string()))
        })
        .collect();

    // Pure-inspection flags (`--validate`, `--list`, `--describe`, `--vars`)
    // should be able to open a bundle without providing every required
    // variable via `--set`; force placeholder-mode for the manifest parse in
    // that case so undefined vars don't abort the flow.
    let inspection_only =
        cli.validate || cli.list || cli.list_vars || cli.describe.is_some() || cli.graph;
    let effective_placeholder = cli.placeholder || inspection_only;

    // Dispatch: determine what kind of input we have and resolve it to a
    // config path + optional BundleCtx.
    let (config_path, mut bundle_ctx) =
        match resolve_input(&raw_config, &vars, effective_placeholder) {
            Ok(resolved) => resolved,
            Err(e) => {
                eprintln!("{}", style::render(&format!("<fr>error:</f> {e}")));
                return ExitCode::FAILURE;
            }
        };

    if cli.list_vars {
        let referenced = match config::scan_vars(&config_path) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("{}", style::render(&format!("<fr>error:</f> {e}")));
                return ExitCode::FAILURE;
            }
        };
        let meta_vars = config::read_meta_vars(&config_path).unwrap_or_default();
        if referenced.is_empty() {
            println!("{}", style::render("<md>no variables referenced</m>"));
            return ExitCode::SUCCESS;
        }
        let name_width = referenced.iter().map(|s| s.len()).max().unwrap_or(0);
        for name in &referenced {
            let default = meta_vars
                .get(name)
                .map(|v| style::render(&format!("<fc>{v}</f>")))
                .or_else(|| {
                    vars.get(name)
                        .map(|v| style::render(&format!("<fc>{v}</f> <md>(from --set)</m>")))
                })
                .unwrap_or_else(|| style::render("<fy>(required)</f>"));
            println!(
                "{}  {default}",
                style::render(&format!("<mb>{name:<name_width$}</m>"))
            );
        }
        return ExitCode::SUCCESS;
    }

    let cfg = match config::parse_config(&config_path, &vars, effective_placeholder) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{}", style::render(&format!("<fr>error:</f> {e}")));
            return ExitCode::FAILURE;
        }
    };

    if cli.validate {
        println!(
            "{}",
            style::render(&format!(
                "<fg>ok:</f> config valid: <mb>{}</m> ({} steps)",
                cfg.name,
                cfg.steps.len()
            ))
        );
        return ExitCode::SUCCESS;
    }

    let index = config::build_step_index(&cfg);

    if cli.list {
        inspect::print_list(&cfg.steps, cli.verbose);
        return ExitCode::SUCCESS;
    }

    if cli.graph {
        let opts = inspect::GraphOpts {
            edges: if cli.edges.is_empty() {
                None
            } else {
                Some(cli.edges.clone())
            },
            label: cli.label,
        };
        if cli.dot {
            inspect::print_graph_dot(&cfg, &opts);
        } else {
            inspect::print_graph(&cfg, &opts);
        }
        return ExitCode::SUCCESS;
    }

    if let Some(id) = &cli.describe {
        match index.get(id) {
            Some(step) => {
                inspect::describe_step(step, &index, cli.depth, cli.verbose);
                return ExitCode::SUCCESS;
            }
            None => {
                eprintln!(
                    "{}",
                    style::render(&format!("<fr>error:</f> no step with id '<fc>{id}</f>'"))
                );
                return ExitCode::FAILURE;
            }
        }
    }

    let mut scope = config::build_scope(&cfg, &vars);
    if let Some(ctx) = bundle_ctx.as_ref() {
        scope.set_bundle_root(ctx.root.to_string_lossy().into_owned());
    }
    let runner = match bundle_ctx.take() {
        Some(ctx) => {
            let mut r = executor::Runner::new_with_bundle(
                index,
                cli.dry_run,
                cli.verbose,
                cfg.meta.clone(),
                scope,
                ctx,
            );
            r.quiet = cli.quiet;
            r.cli_silent = cli.silent;
            r
        }
        None => {
            let mut r =
                executor::Runner::new(index, cli.dry_run, cli.verbose, cfg.meta.clone(), scope);
            r.quiet = cli.quiet;
            r.cli_silent = cli.silent;
            r
        }
    };
    let cwd = std::env::current_dir()
        .map(|p| p.display().to_string())
        .unwrap_or_default();

    if cli.dry_run {
        println!(
            "{}",
            style::render(&format!(
                "<fc>[dry-run]</f> <mb>{}</m> <md>({cwd})</m>",
                cfg.name
            ))
        );
        if let Some(id) = &cli.only {
            match runner.index.get(id) {
                Some(step) => runner.dry_run_audit(&cfg, std::slice::from_ref(step)),
                None => {
                    eprintln!(
                        "{}",
                        style::render(&format!("<fr>error:</f> no step with id '<fc>{id}</f>'"))
                    );
                    return ExitCode::FAILURE;
                }
            }
        } else {
            runner.dry_run_audit(&cfg, &cfg.steps);
        }
        return ExitCode::SUCCESS;
    }

    if cli.quiet < 1 {
        println!(
            "{}",
            style::render(&format!(
                "<fg>Running:</f> <mb>{}</m> <md>({cwd})</m>",
                cfg.name
            ))
        );
    }

    if let Some(id) = &cli.only {
        match runner.index.get(id) {
            Some(step) => {
                if let Err(e) = runner.run_with_deps(step) {
                    return handle_run_error(e, &runner);
                }
            }
            None => {
                eprintln!(
                    "{}",
                    style::render(&format!("<fr>error:</f> no step with id '<fc>{id}</f>'"))
                );
                return ExitCode::FAILURE;
            }
        }
    } else {
        let use_parallel = !cli.no_parallel && (cli.parallel || cfg.meta.parallel);
        let result = if use_parallel {
            runner.run_steps_parallel(&cfg.steps)
        } else {
            runner.run_steps(&cfg.steps)
        };
        if let Err(e) = result {
            return handle_run_error(e, &runner);
        }
    }

    // If we ran from a bundle, mark success (honors Cleanup::OnSuccess) and
    // surface the staging path when we're keeping it around. The actual
    // filesystem cleanup happens when `runner` (and the BundleCtx it owns)
    // drops at the end of main.
    if let Some(bctx) = runner.bundle.as_ref() {
        bctx.mark_success();
        if bctx.will_keep_staging() && cli.quiet < 1 {
            println!(
                "{}",
                style::render(&format!(
                    "<md>bundle staged at:</m> {}",
                    bctx.root.display()
                ))
            );
        }
    }

    if cli.quiet < 1 {
        println!("{}", style::render("<fg>Done.</f>"));
    }
    ExitCode::SUCCESS
}

/// Print the bundle staging path when the ctx will keep it — helpful on
/// failure so users can inspect what got unpacked before the error.
fn handle_run_error(e: executor::ExecError, runner: &executor::Runner) -> ExitCode {
    if let executor::ExecError::EarlyExit { code, message } = e {
        if let Some(msg) = message {
            println!("{}", style::render(&format!("<fc>{msg}</f>")));
        }
        return ExitCode::from(code as u8);
    }
    eprintln!("{}", style::render(&format!("<fr>error:</f> {e}")));
    report_bundle_staging(runner);
    ExitCode::FAILURE
}

fn report_bundle_staging(runner: &executor::Runner) {
    if let Some(bctx) = runner.bundle.as_ref()
        && bctx.will_keep_staging()
    {
        eprintln!(
            "{}",
            style::render(&format!(
                "<md>bundle left staged at:</m> {}",
                bctx.root.display()
            ))
        );
    }
}

fn run_subcommand(cmd: Commands) -> ExitCode {
    match cmd {
        Commands::Pack { src, output } => {
            let output = match output {
                Some(p) => p,
                None => match default_pack_output(&src) {
                    Ok(p) => p,
                    Err(e) => {
                        eprintln!("{}", style::render(&format!("<fr>error:</f> {e}")));
                        return ExitCode::FAILURE;
                    }
                },
            };
            if let Err(e) = bundle::pack(&src, &output) {
                eprintln!("{}", style::render(&format!("<fr>error:</f> {e}")));
                return ExitCode::FAILURE;
            }
            println!(
                "{}",
                style::render(&format!(
                    "<fg>packed:</f> <mb>{}</m> -> <mb>{}</m>",
                    src.display(),
                    output.display()
                ))
            );
            ExitCode::SUCCESS
        }
        Commands::Unpack { archive, output } => {
            let output = match output {
                Some(p) => p,
                None => match default_unpack_output(&archive) {
                    Ok(p) => p,
                    Err(e) => {
                        eprintln!("{}", style::render(&format!("<fr>error:</f> {e}")));
                        return ExitCode::FAILURE;
                    }
                },
            };
            if let Err(e) = bundle::unpack(&archive, &output) {
                eprintln!("{}", style::render(&format!("<fr>error:</f> {e}")));
                return ExitCode::FAILURE;
            }
            println!(
                "{}",
                style::render(&format!(
                    "<fg>unpacked:</f> <mb>{}</m> -> <mb>{}</m>",
                    archive.display(),
                    output.display()
                ))
            );
            ExitCode::SUCCESS
        }
        Commands::Info { archive } => match bundle::info(&archive) {
            Ok(info) => {
                print_bundle_info(&info, &archive);
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("{}", style::render(&format!("<fr>error:</f> {e}")));
                ExitCode::FAILURE
            }
        },
    }
}

/// Derive the default `rig pack` output path: `<basename(src)>.rig` in the
/// current directory. Matches the convention of `tar czf foo.tar.gz foo`
/// producing `foo.tar.gz` in cwd regardless of where `foo` lives.
fn default_pack_output(src: &std::path::Path) -> Result<PathBuf, String> {
    let stem = src
        .file_name()
        .ok_or_else(|| format!("cannot derive output name from {}", src.display()))?;
    let mut out = std::ffi::OsString::from(stem);
    out.push(".rig");
    Ok(PathBuf::from(out))
}

/// Derive the default `rig unpack` output path: archive filename with the
/// `.rig` suffix stripped. Errors if the archive isn't named `*.rig` — the
/// user should pass `--output` explicitly in that case so we don't overwrite
/// or collide with the archive itself.
fn default_unpack_output(archive: &std::path::Path) -> Result<PathBuf, String> {
    let name = archive
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or_else(|| format!("cannot derive output name from {}", archive.display()))?;
    let stripped = name.strip_suffix(".rig").ok_or_else(|| {
        format!(
            "archive {name:?} has no .rig suffix; pass --output to choose a destination explicitly"
        )
    })?;
    if stripped.is_empty() {
        return Err(format!(
            "archive {name:?} would unpack to an empty name; pass --output explicitly"
        ));
    }
    Ok(PathBuf::from(stripped))
}

fn print_bundle_info(info: &bundle::BundleInfo, archive: &std::path::Path) {
    let name = info.name.as_deref().unwrap_or("<unnamed>");
    let version = info.version.as_deref().unwrap_or("?");
    println!(
        "{}",
        style::render(&format!(
            "<mb>{name}</m> <md>v{version}</m> <md>({})</m>",
            archive.display()
        ))
    );
    if let Some(desc) = &info.description {
        println!("{}", style::render(&format!("<md>{desc}</m>")));
    }
    println!(
        "{}",
        style::render(&format!("<mb>steps:</m> {}", info.step_count))
    );
    if let Some(bm) = &info.bundle_meta {
        let cleanup = format!("{:?}", bm.cleanup).to_lowercase();
        println!("{}", style::render(&format!("<mb>cleanup:</m> {cleanup}")));
        if !bm.binary.is_empty() {
            println!(
                "{}",
                style::render(&format!("<mb>binary:</m> {}", bm.binary.join(", ")))
            );
        }
    }

    let files: Vec<&bundle::FileEntry> = info.files.iter().filter(|f| !f.is_dir).collect();
    println!(
        "{}",
        style::render(&format!("<mb>files:</m> {}", files.len()))
    );
    for f in &files {
        println!(
            "  {}",
            style::render(&format!("{}  <md>{} bytes</m>", f.path, f.size))
        );
    }
}
