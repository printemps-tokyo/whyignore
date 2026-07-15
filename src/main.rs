//! Command-line entry point for whyignore.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::Parser;

use whyignore::{evaluate_with_trace, parse_rules, Decision, TraceEntry};

/// Explain which .gitignore rule matches a path, without calling Git.
#[derive(Parser, Debug)]
#[command(name = "whyignore", version, about, long_about = None)]
struct Cli {
    /// The .gitignore (or ignore-style) file to read.
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
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    let text = std::fs::read_to_string(&cli.ignore_file)
        .with_context(|| format!("failed to read {}", cli.ignore_file.display()))?;
    let rules = parse_rules(&text)?;

    let root = cli
        .root
        .clone()
        .or_else(|| cli.ignore_file.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("."));

    let mut results = Vec::new();
    for path in &cli.paths {
        // A trailing slash, or an actual directory on disk, marks a directory.
        let is_dir = path.ends_with('/') || root.join(path.trim_end_matches('/')).is_dir();
        let (decision, trace) = evaluate_with_trace(path, is_dir, &rules);
        results.push((path.clone(), is_dir, decision, trace));
    }

    let ignore_name = cli.ignore_file.display().to_string();
    if cli.json {
        print!("{}", render_json(&results, cli.trace));
    } else {
        print!("{}", render_text(&results, cli.trace, &ignore_name));
    }
    Ok(())
}

type PathResult = (String, bool, Decision, Vec<TraceEntry>);

fn render_text(results: &[PathResult], trace: bool, ignore_name: &str) -> String {
    let width = results
        .iter()
        .map(|(p, _, _, _)| p.len())
        .max()
        .unwrap_or(0)
        .min(48);
    let mut out = String::new();
    for (path, _is_dir, dec, entries) in results {
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
        .map(|(path, is_dir, dec, entries)| {
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
            format!(
                "  {{ \"path\": \"{}\", \"isDir\": {}, \"ignored\": {}, \"rule\": {}{} }}",
                json_escape(path),
                is_dir,
                dec.ignored,
                rule,
                trace_field
            )
        })
        .collect();
    format!("[\n{}\n]\n", items.join(",\n"))
}
