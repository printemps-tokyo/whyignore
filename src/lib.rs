//! whyignore — explain which `.gitignore` rule decides a path.
//!
//! Each gitignore line becomes a [`Rule`] whose glob is compiled to a regex.
//! Evaluating a path walks it directory-by-directory (so directory excludes
//! propagate to their contents and cannot be re-included, matching Git), and
//! within each level the last matching rule wins. Parsing and evaluation are
//! pure, so the behavior is deterministic and unit-tested against documented
//! gitignore semantics. Git itself is never invoked.

use anyhow::{Context, Result};
use regex::Regex;

/// One compiled gitignore rule.
pub struct Rule {
    /// 1-based line number in the source file.
    pub line_no: usize,
    /// The pattern as written (for display), e.g. `build/` or `!keep.log`.
    pub raw: String,
    /// Whether the rule re-includes (a `!` pattern).
    pub negated: bool,
    /// Whether the pattern only matches directories (a trailing `/`).
    pub dir_only: bool,
    regex: Regex,
}

impl Rule {
    /// Whether this rule matches a path prefix that is (or is not) a directory.
    fn matches(&self, prefix: &str, is_dir: bool) -> bool {
        if self.dir_only && !is_dir {
            return false;
        }
        self.regex.is_match(prefix)
    }
}

/// Copy a `[...]` character class from the start of `rest` (which begins with
/// `[`) into `out`, mapping a leading `!` to regex `^`. Returns the number of
/// bytes consumed, or `None` when the class is never closed.
fn copy_char_class(rest: &str, out: &mut String) -> Option<usize> {
    let mut class = String::from("[");
    let mut iter = rest.char_indices().skip(1).peekable();
    if let Some(&(_, c)) = iter.peek() {
        if c == '!' || c == '^' {
            class.push('^');
            iter.next();
        }
    }
    while let Some((j, c)) = iter.next() {
        match c {
            ']' => {
                class.push(']');
                out.push_str(&class);
                return Some(j + 1);
            }
            '\\' => {
                // An escaped character (including `]`) stays inside the class.
                // A backslash at the very end leaves the class unclosed.
                let (_, escaped) = iter.next()?;
                class.push('\\');
                class.push(escaped);
            }
            _ => class.push(c),
        }
    }
    None
}

/// Translate a gitignore glob body into a regex that matches one path string.
///
/// Returns `None` when the pattern can never match anything: git's wildmatch
/// aborts on a pattern that ends in a lone backslash, so such a rule matches
/// no path at all.
fn glob_to_regex(body: &str) -> Option<String> {
    let mut out = String::new();
    let mut rest = body;
    while let Some(c) = rest.chars().next() {
        if let Some(tail) = rest.strip_prefix("**/") {
            out.push_str("(?:.*/)?");
            rest = tail;
            continue;
        }
        if rest == "/**" {
            out.push_str("/.*");
            break;
        }
        if let Some(tail) = rest.strip_prefix("**") {
            out.push_str(".*");
            rest = tail;
            continue;
        }
        match c {
            '*' => out.push_str("[^/]*"),
            '?' => out.push_str("[^/]"),
            '[' => {
                // Copy a character class, mapping a leading `!` to regex `^`.
                if let Some(consumed) = copy_char_class(rest, &mut out) {
                    rest = &rest[consumed..];
                    continue;
                }
                out.push_str("\\["); // no closing bracket: literal
            }
            '\\' => {
                // A backslash escapes the next character: it is matched
                // literally instead of being interpreted as glob syntax.
                let mut chars = rest.chars();
                chars.next();
                match chars.next() {
                    Some(next) => {
                        if "\\.+*?()[]{}|^$".contains(next) {
                            out.push('\\');
                        }
                        out.push(next);
                        rest = &rest[1 + next.len_utf8()..];
                        continue;
                    }
                    // A pattern ending in a lone backslash matches nothing.
                    None => return None,
                }
            }
            _ => {
                if "\\.+(){}|^$".contains(c) {
                    out.push('\\');
                }
                out.push(c);
            }
        }
        rest = &rest[c.len_utf8()..];
    }
    Some(out)
}

/// Parse gitignore text into compiled rules, skipping blanks and comments.
pub fn parse_rules(text: &str) -> Result<Vec<Rule>> {
    let mut rules = Vec::new();
    for (idx, line) in text.lines().enumerate() {
        let trimmed = line.trim_end();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let raw = trimmed.to_string();

        let mut s = trimmed;
        let mut negated = false;
        if let Some(rest) = s.strip_prefix('!') {
            negated = true;
            s = rest;
        } else if let Some(rest) = s.strip_prefix("\\#").or_else(|| s.strip_prefix("\\!")) {
            // Unescape a leading literal '#' or '!'.
            s = &raw[raw.len() - rest.len() - 1..];
        }

        let dir_only = s.ends_with('/');
        let s = s.trim_end_matches('/');
        if s.is_empty() {
            continue;
        }
        let leading_slash = s.starts_with('/');
        let core = s.trim_start_matches('/');
        let anchored = leading_slash || core.contains('/');

        // A pattern that can never match (lone trailing backslash) produces
        // no rule at all, so it never appears in evaluation or --trace output.
        let Some(inner) = glob_to_regex(core) else {
            continue;
        };
        let full = if anchored {
            format!("^{inner}$")
        } else {
            format!("^(?:.*/)?{inner}$")
        };
        let regex =
            Regex::new(&full).with_context(|| format!("bad pattern on line {}", idx + 1))?;
        rules.push(Rule {
            line_no: idx + 1,
            raw,
            negated,
            dir_only,
            regex,
        });
    }
    Ok(rules)
}

/// The rule that decided a path, if any.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatchInfo {
    pub line_no: usize,
    pub raw: String,
    pub negated: bool,
}

/// The outcome of evaluating a path against the rules.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decision {
    pub ignored: bool,
    pub matched: Option<MatchInfo>,
}

/// One rule that matched while evaluating a path, in evaluation order.
///
/// The final entry is always the rule that decided the outcome (the winner),
/// because within each level the last matching rule wins and evaluation stops
/// once a directory exclusion locks the result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TraceEntry {
    pub line_no: usize,
    pub raw: String,
    pub negated: bool,
    /// The path prefix the rule matched (a parent directory or the full path).
    pub prefix: String,
}

/// Normalize a path: drop `./`, and leading/trailing slashes.
fn normalize(path: &str) -> String {
    let p = path.trim();
    let p = p.strip_prefix("./").unwrap_or(p);
    p.trim_matches('/').to_string()
}

/// Decide whether `path` is ignored, and by which rule. `is_dir` marks the
/// final component as a directory (intermediate components are always dirs).
pub fn evaluate(path: &str, is_dir: bool, rules: &[Rule]) -> Decision {
    evaluate_with_trace(path, is_dir, rules).0
}

/// Like [`evaluate`], but also returns every rule that matched during the
/// walk, in evaluation order. Useful for debugging why a negation did or did
/// not take effect.
pub fn evaluate_with_trace(
    path: &str,
    is_dir: bool,
    rules: &[Rule],
) -> (Decision, Vec<TraceEntry>) {
    let norm = normalize(path);
    let comps: Vec<&str> = norm.split('/').filter(|c| !c.is_empty()).collect();

    let mut ignored = false;
    let mut matched: Option<&Rule> = None;
    let mut locked = false;
    let mut prefix = String::new();
    let mut trace = Vec::new();

    for (i, comp) in comps.iter().enumerate() {
        if locked {
            break;
        }
        if i > 0 {
            prefix.push('/');
        }
        prefix.push_str(comp);
        let level_is_dir = if i == comps.len() - 1 { is_dir } else { true };

        // Last matching rule at this level wins.
        let mut here: Option<&Rule> = None;
        for r in rules {
            if r.matches(&prefix, level_is_dir) {
                here = Some(r);
                trace.push(TraceEntry {
                    line_no: r.line_no,
                    raw: r.raw.clone(),
                    negated: r.negated,
                    prefix: prefix.clone(),
                });
            }
        }
        if let Some(r) = here {
            ignored = !r.negated;
            matched = Some(r);
        }
        // A directory excluded here means its contents stay excluded.
        if level_is_dir && ignored {
            locked = true;
        }
    }

    let decision = Decision {
        ignored,
        matched: matched.map(|r| MatchInfo {
            line_no: r.line_no,
            raw: r.raw.clone(),
            negated: r.negated,
        }),
    };
    (decision, trace)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules(text: &str) -> Vec<Rule> {
        parse_rules(text).unwrap()
    }

    fn ignored(path: &str, is_dir: bool, text: &str) -> bool {
        evaluate(path, is_dir, &rules(text)).ignored
    }

    #[test]
    fn basename_pattern_matches_at_any_level() {
        let r = rules("*.log");
        assert!(evaluate("a.log", false, &r).ignored);
        assert!(evaluate("deep/dir/b.log", false, &r).ignored);
        assert!(!evaluate("a.txt", false, &r).ignored);
    }

    #[test]
    fn anchored_pattern_only_matches_at_root() {
        let r = rules("/build");
        assert!(evaluate("build", true, &r).ignored);
        assert!(evaluate("build/out.js", false, &r).ignored); // excluded via parent dir
        assert!(!evaluate("src/build", true, &r).ignored);
    }

    #[test]
    fn directory_only_pattern_needs_a_directory() {
        let r = rules("build/");
        assert!(evaluate("build", true, &r).ignored); // a directory
        assert!(!evaluate("build", false, &r).ignored); // a file named build
        assert!(evaluate("build/app.log", false, &r).ignored); // content of the dir
    }

    #[test]
    fn negation_reincludes_when_parent_is_not_excluded() {
        let text = "*.log\n!keep.log\n";
        assert!(ignored("drop.log", false, text));
        let dec = evaluate("keep.log", false, &rules(text));
        assert!(!dec.ignored);
        assert_eq!(dec.matched.unwrap().line_no, 2);
    }

    #[test]
    fn cannot_reinclude_under_an_excluded_directory() {
        let text = "build/\n!build/keep.log\n";
        let dec = evaluate("build/keep.log", false, &rules(text));
        assert!(dec.ignored);
        assert_eq!(dec.matched.unwrap().line_no, 1); // the directory rule wins
    }

    #[test]
    fn double_star_matches_across_directories() {
        assert!(ignored("a/b/foo", false, "**/foo"));
        assert!(ignored("foo", false, "**/foo"));
        assert!(ignored("a/x/b", false, "a/**/b"));
        assert!(ignored("a/b", false, "a/**/b"));
        // No `b` component, so it does not match.
        assert!(!ignored("a/c", false, "a/**/b"));
    }

    #[test]
    fn last_match_wins() {
        // Re-include then exclude again: the later rule decides.
        let text = "*.log\n!keep.log\nkeep.log\n";
        assert!(ignored("keep.log", false, text));
    }

    #[test]
    fn supports_question_mark_and_classes() {
        assert!(ignored("file1.txt", false, "file?.txt"));
        assert!(!ignored("file10.txt", false, "file?.txt"));
        assert!(ignored("a.md", false, "[abc].md"));
        assert!(!ignored("d.md", false, "[abc].md"));
    }

    #[test]
    fn comments_and_blanks_are_skipped() {
        let r = rules("# a comment\n\n*.tmp\n");
        assert_eq!(r.len(), 1);
        assert!(evaluate("x.tmp", false, &r).ignored);
    }

    #[test]
    fn unmatched_path_is_not_ignored() {
        let dec = evaluate("src/main.rs", false, &rules("*.log\nbuild/\n"));
        assert!(!dec.ignored);
        assert!(dec.matched.is_none());
    }

    #[test]
    fn trace_records_all_matching_rules_in_order() {
        let r = rules("*.log\n!keep.log\n");
        let (dec, trace) = evaluate_with_trace("keep.log", false, &r);
        assert!(!dec.ignored);
        assert_eq!(trace.len(), 2);
        assert_eq!(trace[0].line_no, 1);
        assert!(!trace[0].negated);
        assert_eq!(trace[1].line_no, 2);
        assert!(trace[1].negated);
        // The last trace entry is the winner.
        assert_eq!(dec.matched.unwrap().line_no, trace[1].line_no);
    }

    #[test]
    fn trace_stops_at_an_excluded_directory() {
        let r = rules("build/\n!build/keep.log\n");
        let (dec, trace) = evaluate_with_trace("build/keep.log", false, &r);
        assert!(dec.ignored);
        // Evaluation locks at the excluded parent, so the negation on line 2
        // is never consulted and does not appear in the trace.
        assert_eq!(trace.len(), 1);
        assert_eq!(trace[0].line_no, 1);
        assert_eq!(trace[0].prefix, "build");
    }

    #[test]
    fn trace_is_empty_when_nothing_matches() {
        let (dec, trace) = evaluate_with_trace("src/main.rs", false, &rules("*.log\n"));
        assert!(!dec.ignored);
        assert!(trace.is_empty());
    }

    #[test]
    fn backslash_escapes_a_glob_metacharacter() {
        // `foo\*bar` means a literal `*`, not a wildcard.
        let text = "foo\\*bar";
        assert!(ignored("foo*bar", false, text));
        assert!(!ignored("fooXbar", false, text));
        assert!(!ignored("foobar", false, text));
    }

    #[test]
    fn trailing_lone_backslash_matches_nothing() {
        // Git's wildmatch aborts on `foo\`, so the pattern matches no path;
        // the rule is dropped entirely and never appears in a trace.
        let text = "foo\\";
        let r = rules(text);
        assert!(r.is_empty());
        assert!(!ignored("foo\\", false, text));
        assert!(!ignored("foo", false, text));
    }

    #[test]
    fn escaped_backslash_matches_literal_backslash() {
        // `foo\\` is an escaped backslash: it matches a file named `foo\`.
        let text = "foo\\\\";
        assert!(ignored("foo\\", false, text));
        assert!(!ignored("foo", false, text));
    }

    #[test]
    fn escaped_backslash_then_wildcard() {
        // `foo\\*bar`: a literal backslash followed by a real wildcard.
        let text = "foo\\\\*bar";
        assert!(ignored("foo\\zzbar", false, text));
        assert!(ignored("foo\\bar", false, text)); // `*` may match nothing
        assert!(!ignored("foobar", false, text));
    }

    #[test]
    fn non_ascii_pattern_matches_literally() {
        let text = "foo\u{e9}bar"; // fooébar
        assert!(ignored("foo\u{e9}bar", false, text));
        assert!(!ignored("foobar", false, text));
    }

    #[test]
    fn multibyte_pattern_matches_literally() {
        let text = "日本語.md";
        assert!(ignored("日本語.md", false, text));
        assert!(!ignored("日本語xmd", false, text)); // the dot stays literal
    }

    #[test]
    fn backslash_escapes_a_non_ascii_character() {
        // `foo\é` escapes the é, which simply matches itself.
        let text = "foo\\\u{e9}";
        assert!(ignored("foo\u{e9}", false, text));
        assert!(!ignored("foo", false, text));
    }

    #[test]
    fn escaped_leading_hash_matches_literal_hash() {
        let text = "\\#name";
        assert!(ignored("#name", false, text));
        assert!(!ignored("name", false, text));
    }

    #[test]
    fn escaped_leading_bang_matches_literal_bang() {
        let text = "\\!important";
        let dec = evaluate("!important", false, &rules(text));
        assert!(dec.ignored);
        assert!(!dec.matched.unwrap().negated);
        assert!(!ignored("important", false, text));
    }
}
