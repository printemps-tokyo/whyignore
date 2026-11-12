//! The suggested fixes, checked with Git itself: after applying a fix, the
//! path must no longer be ignored, and its neighbours must stay ignored.

use std::path::Path;
use std::process::Command;

use whyignore::{evaluate_with_trace, parse_rules, suggest_fix, Style};

fn git(dir: &Path, args: &[&str]) -> std::process::Output {
    Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .expect("git runs")
}

fn ignored_by_git(dir: &Path, path: &str) -> bool {
    // check-ignore exits 0 when the path is ignored, 1 when it is not.
    git(dir, &["check-ignore", "-q", "--no-index", path])
        .status
        .code()
        == Some(0)
}

fn apply(text: &str, path: &str, is_dir: bool) -> String {
    let rules = parse_rules(text).unwrap();
    let (decision, trace) = evaluate_with_trace(path, is_dir, &rules);
    let fix = suggest_fix(Style::Git, path, is_dir, &decision, &trace)
        .expect("a fix for an ignored path");
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    match fix.replace_line {
        Some(n) => {
            lines.splice(n - 1..n, fix.lines.clone());
        }
        None => lines.extend(fix.lines.clone()),
    }
    lines.join("\n") + "\n"
}

#[test]
fn fixes_work_in_git() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    assert!(git(d, &["init", "-q"]).status.success());
    for f in [
        "build/keep/a.txt",
        "build/keep/b.txt",
        "build/other.txt",
        "pkg/build/x.txt",
        "x.log",
        "y.log",
    ] {
        let p = d.join(f);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, "x").unwrap();
    }
    let original = "build/\n*.log\n";

    // A file inside an excluded directory.
    let fixed = apply(original, "build/keep/a.txt", false);
    std::fs::write(d.join(".gitignore"), &fixed).unwrap();
    assert!(!ignored_by_git(d, "build/keep/a.txt"), "{fixed}");
    for still in [
        "build/keep/b.txt",
        "build/other.txt",
        "pkg/build/x.txt",
        "x.log",
    ] {
        assert!(
            ignored_by_git(d, still),
            "{still} should stay ignored with:\n{fixed}"
        );
    }
    // And whyignore agrees with Git about the fixed file.
    let rules = parse_rules(&fixed).unwrap();
    assert!(
        !evaluate_with_trace("build/keep/a.txt", false, &rules)
            .0
            .ignored
    );
    assert!(
        evaluate_with_trace("build/other.txt", false, &rules)
            .0
            .ignored
    );

    // A file the rule matches directly.
    let fixed = apply(original, "x.log", false);
    std::fs::write(d.join(".gitignore"), &fixed).unwrap();
    assert!(!ignored_by_git(d, "x.log"), "{fixed}");
    assert!(ignored_by_git(d, "y.log"));
}
