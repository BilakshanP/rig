use crate::config::*;
use crate::style;
use std::collections::{HashMap, HashSet};

pub fn print_list(steps: &[Step], verbose: bool) {
    let id_width = steps
        .iter()
        .map(|s| s.id.as_deref().unwrap_or("-").len())
        .max()
        .unwrap_or(0);

    let name_width = steps.iter().map(|s| s.name.len()).max().unwrap_or(0);

    for step in steps {
        let id = step.id.as_deref().unwrap_or("-");
        let mut flags = Vec::new();
        if step.meta.optional {
            flags.push("optional".to_string());
        }
        if step.meta.fallible {
            flags.push("fallible".to_string());
        }
        if step.meta.sudo {
            flags.push("sudo".to_string());
        }
        if !step.meta.silent.is_empty() {
            let s: Vec<_> = step
                .meta
                .silent
                .iter()
                .map(|s| format!("{s:?}").to_lowercase())
                .collect();
            flags.push(format!("silent: {}", s.join(", ")));
        }
        if let Some(r) = step.meta.retries {
            flags.push(format!("retries: {r}"));
        }

        let flag_str = if flags.is_empty() {
            String::new()
        } else {
            format!(
                "  {}",
                style::render(&format!("<fy>[{}]</f>", flags.join("] [")))
            )
        };

        let desc = if verbose {
            step.description
                .as_deref()
                .map(|d| format!("  {}", style::render(&format!("<md>{d}</m>"))))
                .unwrap_or_default()
        } else {
            String::new()
        };

        println!(
            "{}{flag_str}{desc}",
            style::render(&format!(
                "<fc>{id:<id_width$}</f>  <mb>{:<name_width$}</m>",
                step.name
            ))
        );
    }
}

pub fn describe_step(
    step: &Step,
    index: &HashMap<String, Step>,
    max_depth: Option<u32>,
    verbose: bool,
) {
    let mut seen = HashSet::new();
    describe_inner(step, index, 0, max_depth, verbose, &mut seen);
}

fn describe_inner(
    step: &Step,
    index: &HashMap<String, Step>,
    depth: u32,
    max_depth: Option<u32>,
    verbose: bool,
    seen: &mut HashSet<String>,
) {
    let indent = "  ".repeat(depth as usize);

    if let Some(id) = &step.id
        && !seen.insert(id.clone())
    {
        println!(
            "{indent}{}",
            style::render(&format!("<md>(cycle: {id} already shown)</m>"))
        );
        return;
    }

    let id_str = step
        .id
        .as_deref()
        .map(|id| format!("{id}: "))
        .unwrap_or_default();
    println!(
        "{indent}{}",
        style::render(&format!("<fc>{id_str}</f><mb>{}</m>", step.name))
    );

    if verbose && let Some(desc) = &step.description {
        println!("{indent}  {}", style::render(&format!("<md>{desc}</m>")));
    }

    let ai = format!("{indent}  ");

    match &step.action {
        Action::Shell { commands, dir, env } => {
            let shell = step.meta.shell.clone().unwrap_or_default();
            let base = std::iter::once(shell.cmd.as_str())
                .chain(shell.args.iter().map(|s| s.as_str()))
                .collect::<Vec<_>>()
                .join(" ");
            let prefix = if step.meta.sudo {
                format!("sudo {base}")
            } else {
                base
            };
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
            if *on_conflict != GitOnConflict::Skip {
                println!(
                    "{ai}{}",
                    style::render(&format!("<md>on-conflict:</m> {on_conflict:?}"))
                );
            }
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
                let mut extras = Vec::new();
                if let Some(p) = prompt {
                    extras.push(format!("prompt: {p:?}"));
                }
                if let Some(d) = default {
                    extras.push(format!("default: {d:?}"));
                }
                if *secret {
                    extras.push("secret".to_string());
                }
                let extras = if extras.is_empty() {
                    String::new()
                } else {
                    format!(" ({})", extras.join(", "))
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
        Action::Exit { code, message } => {
            let msg = message.as_deref().unwrap_or("(no message)");
            println!(
                "{ai}{}",
                style::render(&format!("<md>exit {code}:</m> {msg}"))
            );
        }
    }

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

    let mut flags = Vec::new();
    if step.meta.optional {
        flags.push("optional");
    }
    if step.meta.fallible {
        flags.push("fallible");
    }
    if step.meta.sudo {
        flags.push("sudo");
    }
    if !flags.is_empty() {
        println!(
            "{ai}{}",
            style::render(&format!("<md>meta:</m> {}", flags.join(", ")))
        );
    }

    if !step.then.is_empty() {
        let should_expand = max_depth.is_some();
        let can_go_deeper = max_depth.is_none_or(|m| m == 0 || depth + 1 < m);

        if should_expand && can_go_deeper {
            println!("{ai}{}", style::render("<md>then:</m>"));
            for child in &step.then {
                match child {
                    StepRef::Id(id) => {
                        if let Some(s) = index.get(id) {
                            describe_inner(s, index, depth + 1, max_depth, verbose, seen);
                        } else {
                            println!("{ai}  -> {id}");
                        }
                    }
                    StepRef::Inline(s) => {
                        describe_inner(s, index, depth + 1, max_depth, verbose, seen)
                    }
                }
            }
        } else {
            let refs: Vec<_> = step
                .then
                .iter()
                .map(|c| match c {
                    StepRef::Id(id) => id.clone(),
                    StepRef::Inline(s) => format!("[inline: {}]", s.name),
                })
                .collect();
            println!(
                "{ai}{}",
                style::render(&format!("<md>then:</m> {}", refs.join(", ")))
            );
        }
    }
}

fn step_ref_label(sr: &StepRef) -> String {
    match sr {
        StepRef::Id(id) => id.clone(),
        StepRef::Inline(s) => format!("[inline: {}]", s.name),
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

/// Edge in the execution graph.
struct Edge {
    from: String,
    to: String,
    label: String,
}

/// Collect all edges from the config (sequential, depends-on, then, handlers).
fn collect_edges(cfg: &crate::config::Config) -> Vec<Edge> {
    let mut edges = Vec::new();
    let mut prev_id: Option<String> = None;

    for step in &cfg.steps {
        let from = step
            .id
            .clone()
            .unwrap_or_else(|| format!("[{}]", step.name));

        // Sequential order
        if !step.meta.optional
            && let Some(prev) = &prev_id
        {
            edges.push(Edge {
                from: prev.clone(),
                to: from.clone(),
                label: "seq".into(),
            });
        }

        // depends-on
        for dep in &step.depends_on {
            edges.push(Edge {
                from: from.clone(),
                to: dep.clone(),
                label: "depends-on".into(),
            });
        }

        // then
        for child in &step.then {
            if let StepRef::Id(id) = child {
                edges.push(Edge {
                    from: from.clone(),
                    to: id.clone(),
                    label: "then".into(),
                });
            }
        }

        // on-success
        if let Some(refs) = &step.on_success {
            for sr in refs {
                if let StepRef::Id(id) = sr {
                    edges.push(Edge {
                        from: from.clone(),
                        to: id.clone(),
                        label: "on-success".into(),
                    });
                }
            }
        }

        // on-failure
        if let Some(refs) = &step.on_failure {
            for sr in refs {
                if let StepRef::Id(id) = sr {
                    edges.push(Edge {
                        from: from.clone(),
                        to: id.clone(),
                        label: "on-failure".into(),
                    });
                }
            }
        }

        // on-return
        if let Some(map) = &step.on_return {
            for (code, refs) in map {
                for sr in refs {
                    if let StepRef::Id(id) = sr {
                        edges.push(Edge {
                            from: from.clone(),
                            to: id.clone(),
                            label: format!("on-return({code})"),
                        });
                    }
                }
            }
        }

        if !step.meta.optional {
            prev_id = Some(from);
        }
    }

    edges
}

/// Options for graph rendering.
pub struct GraphOpts {
    /// If Some, only include edges whose label matches one of these.
    pub edges: Option<Vec<String>>,
    /// Whether to show labels on edges.
    pub label: bool,
}

impl GraphOpts {
    fn include(&self, label: &str) -> bool {
        match &self.edges {
            None => true,
            Some(filter) => filter
                .iter()
                .any(|f| f == "all" || label.starts_with(f.as_str())),
        }
    }
}

/// Print the execution graph as ASCII.
pub fn print_graph(cfg: &crate::config::Config, opts: &GraphOpts) {
    let edges: Vec<_> = collect_edges(cfg)
        .into_iter()
        .filter(|e| opts.include(&e.label))
        .collect();

    println!(
        "{}",
        style::render(&format!("<mb>{}</m> <md>execution graph</m>", cfg.name))
    );
    println!();

    let mut by_source: HashMap<String, Vec<(&str, &str)>> = HashMap::new();
    for e in &edges {
        by_source
            .entry(e.from.clone())
            .or_default()
            .push((&e.to, &e.label));
    }

    for step in &cfg.steps {
        let id = step
            .id
            .clone()
            .unwrap_or_else(|| format!("[{}]", step.name));
        let display = step.id.as_deref().unwrap_or(&step.name);
        println!("  {}", style::render(&format!("<fc>{display}</f>")));
        if let Some(targets) = by_source.get(&id) {
            for (to, label) in targets {
                if opts.label {
                    println!("    {to} ({label})");
                } else {
                    println!("    {to}");
                }
            }
        }
    }
}

/// Print the execution graph in Graphviz DOT format.
pub fn print_graph_dot(cfg: &crate::config::Config, opts: &GraphOpts) {
    let edges: Vec<_> = collect_edges(cfg)
        .into_iter()
        .filter(|e| opts.include(&e.label))
        .collect();

    println!("digraph \"{}\" {{", cfg.name);
    println!("    rankdir=LR;");
    println!("    node [shape=box];");

    for step in &cfg.steps {
        let id = step
            .id
            .clone()
            .unwrap_or_else(|| format!("[{}]", step.name));
        println!("    \"{}\" [label=\"{}\"];", id, step.name);
    }

    for e in &edges {
        let style = match e.label.as_str() {
            "seq" => "style=solid",
            "then" => "style=bold",
            "depends-on" => "style=dashed",
            _ => "style=dotted",
        };
        if opts.label {
            println!(
                "    \"{}\" -> \"{}\" [label=\"{}\", {}];",
                e.from, e.to, e.label, style
            );
        } else {
            println!("    \"{}\" -> \"{}\" [{}];", e.from, e.to, style);
        }
    }

    println!("}}");
}
