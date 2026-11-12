//! Command-line entry point for whyignore.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::Parser;

use whyignore::docker::{docker_evaluate, parse_docker_rules};
use whyignore::{
    evaluate_with_trace, npm_note, parse_rules, suggest_fix, Decision, Fix, Style, TraceEntry,
};

/// Explain which .gitignore rule matches a path, without calling Git.
#[derive(Parser, Debug)]
#[command(name = "whyignore", version, about, long_about = None)]
struct Cli {
    /// The ignore file to read: .gitignore (or another gitignore-style file),
    /// .dockerignore, or .npmignore.
    ignore_file: PathBuf,

    /// One or more paths to test (relative to the ignore file's directory).
    #[arg(required = true)]
    paths: Vec<String>,

    /// Base directory used to tell whether a path is a directory on disk.
    #[arg(long)]
    root: Option<PathBuf>,

    /// Output JSON instead of the human-readable format.
    #[arg(long)]
    json: bool,

    /// Show every matching rule in evaluation order, not just the winner.
    #[arg(long)]
    trace: bool,

    /// Whose rules the file follows (default: from its name; .dockerignore
    /// is docker, .npmignore is npm, anything else git).
    #[arg(long, value_parser = ["git", "docker", "npm"])]
    style: Option<String>,

    /// Do not suggest how to include an ignored path.
    #[arg(long)]
    no_fix: bool,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    let text = std::fs::read_to_string(&cli.ignore_file)
        .with_context(|| format!("failed to read {}", cli.ignore_file.display()))?;
    let style = match cli.style.as_deref() {
        Some("docker") => Style::Docker,
        Some("npm") => Style::Npm,
        Some(_) => Style::Git,
        None => Style::for_file(
            &cli.ignore_file
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
        ),
    };
    let rules = if style == Style::Docker {
        Vec::new()
    } else {
        parse_rules(&text)?
    };
    let docker_rules = if style == Style::Docker {
        parse_docker_rules(&text)?
    } else {
        Vec::new()
    };

    let root = cli
        .root
        .clone()
        .or_else(|| cli.ignore_file.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("."));

    let mut results = Vec::new();
    for path in &cli.paths {
        // A trailing slash, or an actual directory on disk, marks a directory.
        let is_dir = path.ends_with('/') || root.join(path.trim_end_matches('/')).is_dir();
        let (decision, trace) = if style == Style::Docker {
            docker_evaluate(path, &docker_rules)
        } else {
            evaluate_with_trace(path, is_dir, &rules)
        };
        let fix = if cli.no_fix {
            None
        } else {
            suggest_fix(style, path, is_dir, &decision, &trace)
        };
        let note = if style == Style::Npm {
            npm_note(path)
        } else {
            None
        };
        results.push(PathResult {
            path: path.clone(),
            is_dir,
            decision,
            trace,
            fix,
            note,
        });
    }

    let ignore_name = cli.ignore_file.display().to_string();
    if cli.json {
        print!("{}", render_json(&results, cli.trace));
    } else {
        print!("{}", render_text(&results, cli.trace, &ignore_name));
    }
    Ok(())
}

struct PathResult {
    path: String,
    is_dir: bool,
    decision: Decision,
    trace: Vec<TraceEntry>,
    fix: Option<Fix>,
    note: Option<&'static str>,
}

fn render_text(results: &[PathResult], trace: bool, ignore_name: &str) -> String {
    let width = results
        .iter()
        .map(|r| r.path.len())
        .max()
        .unwrap_or(0)
        .min(48);
    let mut out = String::new();
    for r in results {
        let (path, dec, entries) = (&r.path, &r.decision, &r.trace);
        let label = if dec.ignored {
            "IGNORED"
        } else {
            "not ignored"
        };
        let detail = match &dec.matched {
            Some(m) if dec.ignored => format!("line {}: {}", m.line_no, m.raw),
            Some(m) => format!("re-included by line {}: {}", m.line_no, m.raw),
            None => "no matching rule".to_string(),
        };
        out.push_str(&format!("{path:<width$}  {label:<11}  {detail}\n"));
        if let Some(note) = r.note {
            out.push_str(&format!("    note: {note}\n"));
        }
        if let Some(fix) = &r.fix {
            out.push_str(&format!("    to include it: {}\n", fix.note));
            match fix.replace_line {
                Some(n) => out.push_str(&format!("      replace line {n} with:\n")),
                None => out.push_str("      add at the end:\n"),
            }
            for l in &fix.lines {
                out.push_str(&format!("        {l}\n"));
            }
        }
        if trace {
            if entries.is_empty() {
                out.push_str("    (no rules matched)\n");
            }
            for (i, t) in entries.iter().enumerate() {
                let effect = if t.negated { "re-include" } else { "ignore" };
                let winner = if i + 1 == entries.len() {
                    "  <- winner"
                } else {
                    ""
                };
                out.push_str(&format!(
                    "    {ignore_name}:{}  {}  ({effect}, matched \"{}\"){winner}\n",
                    t.line_no, t.raw, t.prefix
                ));
            }
        }
    }
    out
}

fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

fn render_json(results: &[PathResult], trace: bool) -> String {
    let items: Vec<String> = results
        .iter()
        .map(|r| {
            let (path, is_dir, dec, entries) = (&r.path, r.is_dir, &r.decision, &r.trace);
            let rule = match &dec.matched {
                Some(m) => format!(
                    "{{ \"line\": {}, \"pattern\": \"{}\", \"negated\": {} }}",
                    m.line_no,
                    json_escape(&m.raw),
                    m.negated
                ),
                None => "null".to_string(),
            };
            let trace_field = if trace {
                let steps: Vec<String> = entries
                    .iter()
                    .enumerate()
                    .map(|(i, t)| {
                        format!(
                            "{{ \"line\": {}, \"pattern\": \"{}\", \"negated\": {}, \"matched\": \"{}\", \"winner\": {} }}",
                            t.line_no,
                            json_escape(&t.raw),
                            t.negated,
                            json_escape(&t.prefix),
                            i + 1 == entries.len()
                        )
                    })
                    .collect();
                format!(", \"trace\": [{}]", steps.join(", "))
            } else {
                String::new()
            };
            let fix_field = match &r.fix {
                Some(f) => format!(
                    ", \"fix\": {{ \"note\": \"{}\", \"replaceLine\": {}, \"lines\": [{}] }}",
                    json_escape(&f.note),
                    f.replace_line.map_or("null".to_string(), |n| n.to_string()),
                    f.lines.iter().map(|l| format!("\"{}\"", json_escape(l))).collect::<Vec<_>>().join(", ")
                ),
                None => String::new(),
            };
            let note_field = r.note.map_or(String::new(), |n| format!(", \"note\": \"{}\"", json_escape(n)));
            format!(
                "  {{ \"path\": \"{}\", \"isDir\": {}, \"ignored\": {}, \"rule\": {}{}{}{} }}",
                json_escape(path),
                is_dir,
                dec.ignored,
                rule,
                trace_field,
                fix_field,
                note_field
            )
        })
        .collect();
    format!("[\n{}\n]\n", items.join(",\n"))
}
