//! `.dockerignore` matching, ported from moby/patternmatcher (which Docker's
//! CLI and BuildKit use): patterns are rooted at the build context, `*` and
//! `?` stay within one path component, `**` spans any number of them, a
//! pattern matching a parent directory matches everything inside it, and the
//! last matching line wins, so `!` can re-include a file even inside an
//! excluded directory (unlike Git).

use anyhow::{Context, Result};
use regex::Regex;

use crate::{Decision, MatchInfo, TraceEntry};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Exact,
    Prefix,
    Suffix,
    Regexp,
}

/// One `.dockerignore` line, cleaned the way Docker cleans it.
pub struct DockerRule {
    pub line_no: usize,
    pub raw: String,
    pub negated: bool,
    /// The pattern after trimming, `filepath.Clean` and dropping a leading `/`.
    pub cleaned: String,
    kind: Kind,
    regex: Option<Regex>,
}

/// Go's `path.Clean`: collapse slashes, drop `.`, resolve `..`, no trailing slash.
fn clean(p: &str) -> String {
    let rooted = p.starts_with('/');
    let mut parts: Vec<&str> = Vec::new();
    for c in p.split('/') {
        match c {
            "" | "." => {}
            ".." => {
                if parts.last().is_some_and(|l| *l != "..") {
                    parts.pop();
                } else if !rooted {
                    parts.push("..");
                }
            }
            _ => parts.push(c),
        }
    }
    let body = parts.join("/");
    match (rooted, body.is_empty()) {
        (true, _) => format!("/{body}"),
        (false, true) => ".".to_string(),
        (false, false) => body,
    }
}

/// Characters Go's `filepath.Match` gives no meaning that are regex syntax.
fn should_escape(c: char) -> bool {
    matches!(c, '.' | '+' | '(' | ')' | '|' | '{' | '}' | '$')
}

fn compile(pattern: &str) -> Result<(Kind, Option<Regex>)> {
    let mut re = String::from("^");
    let mut kind = Kind::Exact;
    let chars: Vec<char> = pattern.chars().collect();
    let mut i = 0;
    let mut first = true;
    while i < chars.len() {
        let ch = chars[i];
        i += 1;
        if ch == '*' {
            if chars.get(i) == Some(&'*') {
                i += 1;
                if chars.get(i) == Some(&'/') {
                    i += 1;
                }
                if i >= chars.len() {
                    if kind == Kind::Exact {
                        kind = Kind::Prefix;
                    } else {
                        re.push_str(".*");
                        kind = Kind::Regexp;
                    }
                } else {
                    re.push_str("(.*/)?");
                    kind = Kind::Regexp;
                }
                if first {
                    kind = Kind::Suffix;
                }
            } else {
                re.push_str("[^/]*");
                kind = Kind::Regexp;
            }
        } else if ch == '?' {
            re.push_str("[^/]");
            kind = Kind::Regexp;
        } else if should_escape(ch) {
            re.push('\\');
            re.push(ch);
        } else if ch == '\\' {
            match chars.get(i) {
                Some(&next) => {
                    i += 1;
                    re.push_str(&regex::escape(&next.to_string()));
                    kind = Kind::Regexp;
                }
                None => re.push_str("\\\\"),
            }
        } else if ch == '[' || ch == ']' {
            re.push(ch);
            kind = Kind::Regexp;
        } else {
            re.push_str(&regex::escape(&ch.to_string()));
        }
        first = false;
    }
    if kind == Kind::Regexp {
        re.push('$');
        let compiled = Regex::new(&re).with_context(|| format!("invalid pattern {pattern:?}"))?;
        return Ok((kind, Some(compiled)));
    }
    Ok((kind, None))
}

impl DockerRule {
    fn matches(&self, path: &str) -> bool {
        match self.kind {
            Kind::Exact => path == self.cleaned,
            Kind::Prefix => path.starts_with(&self.cleaned[..self.cleaned.len() - 2]),
            Kind::Suffix => {
                let suffix = &self.cleaned[2..];
                path.ends_with(suffix) || (suffix.starts_with('/') && path == &suffix[1..])
            }
            Kind::Regexp => self.regex.as_ref().is_some_and(|r| r.is_match(path)),
        }
    }
}

/// Parse a `.dockerignore`, as Docker's `ignorefile.ReadAll` and pattern
/// matcher read it. The pattern `.` is skipped, as Docker skips it.
pub fn parse_docker_rules(text: &str) -> Result<Vec<DockerRule>> {
    let mut rules = Vec::new();
    for (idx, line) in text.lines().enumerate() {
        let line = if idx == 0 {
            line.trim_start_matches('\u{feff}')
        } else {
            line
        };
        if line.starts_with('#') {
            continue;
        }
        let mut pattern = line.trim();
        if pattern.is_empty() {
            continue;
        }
        let negated = pattern.starts_with('!');
        if negated {
            pattern = pattern[1..].trim();
        }
        let mut cleaned = clean(pattern);
        if cleaned.len() > 1 && cleaned.starts_with('/') {
            cleaned.remove(0);
        }
        if cleaned == "." {
            continue;
        }
        let (kind, regex) = compile(&cleaned)?;
        rules.push(DockerRule {
            line_no: idx + 1,
            raw: line.trim().to_string(),
            negated,
            cleaned,
            kind,
            regex,
        });
    }
    Ok(rules)
}

/// Decide `path` the way Docker does: each rule matches the path or one of
/// its parent directories, and the last such rule wins.
pub fn docker_evaluate(path: &str, rules: &[DockerRule]) -> (Decision, Vec<TraceEntry>) {
    let norm = clean(path.trim().trim_start_matches("./"));
    let norm = norm.trim_start_matches('/').to_string();
    let parts: Vec<&str> = norm.split('/').collect();
    let mut trace = Vec::new();
    let mut winner: Option<&DockerRule> = None;
    for rule in rules {
        let mut hit = rule.matches(&norm).then(|| norm.clone());
        if hit.is_none() {
            for i in 1..parts.len() {
                let parent = parts[..i].join("/");
                if rule.matches(&parent) {
                    hit = Some(parent);
                    break;
                }
            }
        }
        if let Some(prefix) = hit {
            trace.push(TraceEntry {
                line_no: rule.line_no,
                raw: rule.raw.clone(),
                negated: rule.negated,
                prefix,
            });
            winner = Some(rule);
        }
    }
    let decision = Decision {
        ignored: winner.is_some_and(|r| !r.negated),
        matched: winner.map(|r| MatchInfo {
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

    fn ignored(text: &str, path: &str) -> bool {
        docker_evaluate(path, &parse_docker_rules(text).unwrap())
            .0
            .ignored
    }

    #[test]
    fn patterns_are_rooted_unlike_git() {
        assert!(ignored("*.md", "README.md"));
        assert!(!ignored("*.md", "docs/README.md"));
        assert!(ignored("**/*.md", "docs/a/README.md"));
        assert!(ignored("**/*.md", "README.md"));
    }

    #[test]
    fn the_examples_from_the_docker_docs() {
        let text = "# comment\n*/temp*\n*/*/temp*\ntemp?\n";
        assert!(ignored(text, "somedir/temporary.txt"));
        assert!(ignored(text, "somedir/temp"));
        assert!(ignored(text, "somedir/subdir/temporary.txt"));
        assert!(ignored(text, "tempa"));
        assert!(!ignored(text, "tempab"));
        // Leading and trailing slashes do not matter.
        for p in ["/foo/bar/", "/foo/bar", "foo/bar/", "foo/bar"] {
            assert!(ignored(p, "foo/bar"), "{p}");
            assert!(ignored(p, "foo/bar/x.txt"), "{p}");
        }
    }

    #[test]
    fn last_matching_line_wins_and_can_reinclude_inside_a_directory() {
        assert!(!ignored("*.md\n!README.md\n", "README.md"));
        let a = "*.md\n!README*.md\nREADME-secret.md\n";
        assert!(ignored(a, "README-secret.md"));
        assert!(!ignored(a, "README-public.md"));
        let b = "*.md\nREADME-secret.md\n!README*.md\n";
        assert!(!ignored(b, "README-secret.md"));
        // Unlike Git, a file inside an excluded directory can come back.
        assert!(!ignored(
            "node_modules\n!node_modules/keep/index.js\n",
            "node_modules/keep/index.js"
        ));
    }

    #[test]
    fn double_star_forms() {
        assert!(ignored("foo/**", "foo/a/b"));
        assert!(ignored("**/node_modules", "a/b/node_modules/x"));
        assert!(ignored("a/**/b", "a/b"));
        assert!(ignored("a/**/b", "a/x/y/b"));
        assert!(!ignored("a/**/b", "a/xb"));
        // A lone "." is skipped, as Docker skips it.
        assert!(!ignored(".\n", "anything"));
    }

    #[test]
    fn clean_matches_go() {
        assert_eq!(clean("a//b/./c/.."), "a/b");
        assert_eq!(clean("/../x"), "/x");
        assert_eq!(clean("../x"), "../x");
        assert_eq!(clean("./"), ".");
    }
}
