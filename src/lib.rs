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

/// Translate a gitignore glob body into a regex that matches one path string.
fn glob_to_regex(body: &str) -> String {
    let bytes = body.as_bytes();
    let mut out = String::new();
    let mut i = 0;
    let n = bytes.len();
    while i < n {
        let rest = &body[i..];
        if let Some(stripped) = rest.strip_prefix("**/") {
            let _ = stripped;
            out.push_str("(?:.*/)?");
            i += 3;
        } else if rest == "/**" {
            out.push_str("/.*");
            i += 3;
        } else if rest.starts_with("**") {
            out.push_str(".*");
            i += 2;
        } else {
            let c = bytes[i] as char;
            match c {
                '*' => out.push_str("[^/]*"),
                '?' => out.push_str("[^/]"),
                '[' => {
                    // Copy a character class, mapping a leading `!` to regex `^`.
                    let mut j = i + 1;
                    let mut class = String::from("[");
                    if j < n && (bytes[j] == b'!' || bytes[j] == b'^') {
                        class.push('^');
                        j += 1;
                    }
                    while j < n && bytes[j] != b']' {
                        if bytes[j] == b'\\' && j + 1 < n {
                            class.push('\\');
                            class.push(bytes[j + 1] as char);
                            j += 2;
                        } else {
                            class.push(bytes[j] as char);
                            j += 1;
                        }
                    }
                    if j < n {
                        class.push(']');
                        out.push_str(&class);
                        i = j + 1;
                        continue;
                    }
                    out.push_str("\\["); // no closing bracket: literal
                }
                '\\' => {
                    // A backslash escapes the next character: it is matched
                    // literally instead of being interpreted as glob syntax.
                    if i + 1 < n {
                        let next = bytes[i + 1] as char;
                        if "\\.+*?()[]{}|^$".contains(next) {
                            out.push('\\');
                        }
                        out.push(next);
                        i += 2;
                        continue;
                    }
                    // A trailing backslash matches a literal backslash.
                    out.push_str("\\\\");
                }
                _ => {
                    if "\\.+(){}|^$".contains(c) {
                        out.push('\\');
                    }
                    out.push(c);
                }
            }
            i += 1;
        }
    }
    out
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

        let inner = glob_to_regex(core);
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

/// Normalize a path: drop `./`, and leading/trailing slashes.
fn normalize(path: &str) -> String {
    let p = path.trim();
    let p = p.strip_prefix("./").unwrap_or(p);
    p.trim_matches('/').to_string()
}

/// Decide whether `path` is ignored, and by which rule. `is_dir` marks the
/// final component as a directory (intermediate components are always dirs).
pub fn evaluate(path: &str, is_dir: bool, rules: &[Rule]) -> Decision {
    let norm = normalize(path);
    let comps: Vec<&str> = norm.split('/').filter(|c| !c.is_empty()).collect();

    let mut ignored = false;
    let mut matched: Option<&Rule> = None;
    let mut locked = false;
    let mut prefix = String::new();

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

    Decision {
        ignored,
        matched: matched.map(|r| MatchInfo {
            line_no: r.line_no,
            raw: r.raw.clone(),
            negated: r.negated,
        }),
    }
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
    fn backslash_escapes_a_glob_metacharacter() {
        // `foo\*bar` means a literal `*`, not a wildcard.
        let text = "foo\\*bar";
        assert!(ignored("foo*bar", false, text));
        assert!(!ignored("fooXbar", false, text));
        assert!(!ignored("foobar", false, text));
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
