use crate::config::*;
use crate::style;
use std::collections::{HashMap, HashSet};

pub fn print_list(steps: &[Step], verbose: bool) {
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

pub fn describe_step(step: &Step, index: &HashMap<String, Step>, max_depth: Option<u32>, verbose: bool) {
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
        println!("{indent}{}", style::render(&format!("<md>(cycle: {id} already shown)</m>")));
        return;
    }

    let id_str = step.id.as_deref().map(|id| format!("{id}: ")).unwrap_or_default();
    println!("{indent}{}", style::render(&format!("<fc>{id_str}</f><mb>{}</m>", step.name)));

    if verbose
        && let Some(desc) = &step.description
    {
        println!("{indent}  {}", style::render(&format!("<md>{desc}</m>")));
    }

    let ai = format!("{indent}  ");

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
                FsOp::Create { path, recurse, content, expand } => {
                    for p in path {
                        let kind = if p.ends_with('/') { "dir" } else { "file" };
                        println!("{ai}{}", style::render(&format!("<md>create {kind}:</m> {p}")));
                    }
                    if *recurse { println!("{ai}{}", style::render("<md>recurse:</m> true")); }
                    if let Some(c) = content { println!("{ai}{}", style::render(&format!("<md>content:</m> {c:?}"))); }
                    if let Some(label) = expand_label(expand) { println!("{ai}{}", style::render(&format!("<md>expand:</m> {label}"))); }
                }
                FsOp::Symlink { from, to, expand } => {
                    println!("{ai}{}", style::render(&format!("<md>symlink</m> {from} -> {to}")));
                    if let Some(label) = expand_label(expand) { println!("{ai}{}", style::render(&format!("<md>expand:</m> {label}"))); }
                }
                FsOp::Copy { from, to, expand } => {
                    println!("{ai}{}", style::render(&format!("<md>copy</m> {from} -> {to}")));
                    if let Some(label) = expand_label(expand) { println!("{ai}{}", style::render(&format!("<md>expand:</m> {label}"))); }
                }
                FsOp::Move { from, to, expand } => {
                    println!("{ai}{}", style::render(&format!("<md>move</m> {from} -> {to}")));
                    if let Some(label) = expand_label(expand) { println!("{ai}{}", style::render(&format!("<md>expand:</m> {label}"))); }
                }
                FsOp::Delete { path, recurse, expand } => {
                    for p in path { println!("{ai}{}", style::render(&format!("<md>delete:</m> {p}"))); }
                    if *recurse { println!("{ai}{}", style::render("<md>recurse:</m> true")); }
                    if let Some(label) = expand_label(expand) { println!("{ai}{}", style::render(&format!("<md>expand:</m> {label}"))); }
                }
            }
            if let Some(c) = if_exists { println!("{ai}{}", style::render(&format!("<md>if-exists:</m> {}", condition_label(c)))); }
            if let Some(c) = if_not_exists { println!("{ai}{}", style::render(&format!("<md>if-not-exists:</m> {}", condition_label(c)))); }
        }
        Action::Io { op } => match op {
            IoOp::Write { level, message, markup } => {
                let ml = if *markup { " [markup]" } else { "" };
                println!("{ai}{}", style::render(&format!("<md>{level:?}:</m> {message:?}{ml}")));
            }
            IoOp::Read { read, prompt, default, secret } => {
                let mut extras = Vec::new();
                if let Some(p) = prompt { extras.push(format!("prompt: {p:?}")); }
                if let Some(d) = default { extras.push(format!("default: {d:?}")); }
                if *secret { extras.push("secret".to_string()); }
                let extras = if extras.is_empty() { String::new() } else { format!(" ({})", extras.join(", ")) };
                println!("{ai}{}", style::render(&format!("<md>read:</m> {read}{extras}")));
            }
        }
        Action::Var { name, source } => {
            match source {
                VarSource::From { from } => println!("{ai}{}", style::render(&format!("<md>var {name} \\<-</m> {}", step_ref_label(from)))),
                VarSource::To { to } => println!("{ai}{}", style::render(&format!("<md>var {name} -></m> {}", step_ref_label(to)))),
                VarSource::Command { command } => println!("{ai}{}", style::render(&format!("<md>var {name} :=</m> {command:?}"))),
                VarSource::File { file } => println!("{ai}{}", style::render(&format!("<md>var {name} \\<- file</m> {file:?}"))),
            }
        }
    }

    if let Some(refs) = &step.on_success { println!("{ai}{}", style::render(&format!("<md>on-success:</m> {}", step_refs_label(refs)))); }
    if let Some(refs) = &step.on_failure { println!("{ai}{}", style::render(&format!("<md>on-failure:</m> {}", step_refs_label(refs)))); }
    if let Some(map) = &step.on_return {
        println!("{ai}{}", style::render("<md>on-return:</m>"));
        for (code, refs) in map { println!("{ai}  {}", style::render(&format!("{code} -> <fc>{}</f>", step_refs_label(refs)))); }
    }

    let mut flags = Vec::new();
    if step.meta.optional { flags.push("optional"); }
    if step.meta.fallible { flags.push("fallible"); }
    if step.meta.sudo { flags.push("sudo"); }
    if !flags.is_empty() {
        println!("{ai}{}", style::render(&format!("<md>meta:</m> {}", flags.join(", "))));
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
                    StepRef::Inline(s) => describe_inner(s, index, depth + 1, max_depth, verbose, seen),
                }
            }
        } else {
            let refs: Vec<_> = step.then.iter().map(|c| match c {
                StepRef::Id(id) => id.clone(),
                StepRef::Inline(s) => format!("[inline: {}]", s.name),
            }).collect();
            println!("{ai}{}", style::render(&format!("<md>then:</m> {}", refs.join(", "))));
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
    refs.iter().map(step_ref_label).collect::<Vec<_>>().join(", ")
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
    if flags.from { parts.push("from"); }
    if flags.to { parts.push("to"); }
    if flags.path { parts.push("path"); }
    if flags.contents { parts.push("contents"); }
    Some(parts.join(", "))
}
