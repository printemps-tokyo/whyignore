# whyignore

> Explain which `.gitignore` rule matches a path, without calling Git. Fast, offline Rust CLI.

[![CI](https://github.com/printemps-tokyo/whyignore/actions/workflows/ci.yml/badge.svg)](https://github.com/printemps-tokyo/whyignore/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](./LICENSE)

`whyignore` answers "why is this file ignored (or not)?" by telling you exactly
which line of a `.gitignore` decides each path:

```console
$ whyignore .gitignore build/app.log keep.log docs/README.md src/main.rs
build/app.log     IGNORED      line 2: build/
keep.log          not ignored  re-included by line 4: !keep.log
docs/README.md    not ignored  re-included by line 7: !docs/README.md
src/main.rs       not ignored  no matching rule
```

It implements gitignore semantics directly (anchoring, `**`, negation,
directory rules, last-match-wins) and never shells out to Git, so it works on
arbitrary ignore files and paths that do not exist yet.

## Requirements

- A Rust toolchain (to build from source)

## Install

```bash
cargo install --git https://github.com/printemps-tokyo/whyignore
# or, from a clone:
cargo install --path .
```

Tagged [releases](https://github.com/printemps-tokyo/whyignore/releases) carry prebuilt binaries for Linux (x86_64)
and macOS (x86_64, Apple silicon), with SHA-256 checksums.

## Usage

```bash
whyignore <ignore-file> <path...>
whyignore .gitignore build/ src/index.ts node_modules/x
whyignore --root . .gitignore dist            # check dir-ness on disk under --root
whyignore .gitignore docs/guide.md --json
whyignore .gitignore keep.log --trace         # show every rule that matched
whyignore .dockerignore docs/a.md             # Docker's rules (from the file name)
whyignore .npmignore package-lock.json        # npm's rules and fixed lists
```

| Option | Description |
| --- | --- |
| `<ignore-file>` | The ignore file to read: `.gitignore` (or gitignore-style), `.dockerignore`, `.npmignore` |
| `<path...>` | One or more paths to test, relative to the ignore file's directory |
| `--root <dir>` | Base directory used to tell whether a path is a directory on disk |
| `--json` | Output JSON instead of the human-readable format |
| `--trace` | Show every matching rule in evaluation order, not just the winner |
| `--style <git\|docker\|npm>` | Whose rules the file follows (default: from its name) |
| `--no-fix` | Do not suggest how to include an ignored path |

### How to include an ignored path

For each ignored path, whyignore says what to change:

```console
$ whyignore .gitignore build/keep/a.txt x.log
build/keep/a.txt  IGNORED      line 1: build/
    to include it: line 1 `build/` excludes the directory `build/`, and Git cannot re-include a path inside an excluded directory; exclude its contents instead
      replace line 1 with:
        **/build/**
        !/build/**/
        !/build/keep/a.txt
x.log             IGNORED      line 2: *.log
    to include it: line 2 `*.log` matches the path itself; a later ! line re-includes it
      add at the end:
        !/x.log
```

When the deciding rule matches the path itself, a `!` line at the end brings it
back. When it excludes a parent directory, Git never looks inside it, so the
fix excludes the directory's contents instead, re-includes the directories
below it, and then the path; other files stay ignored. The tests apply these
fixes and check them with `git check-ignore`. With `--json`, a result carries
`fix: { note, replaceLine, lines }`.

### .dockerignore and .npmignore

- `.dockerignore` (and `*.dockerignore`) follows Docker's rules, ported from
  moby/patternmatcher: patterns are rooted at the build context (`*.md` is
  only the top level; use `**/*.md`), a pattern matching a parent directory
  matches everything in it, and the last matching line wins, so a `!` line can
  re-include a file even inside an excluded directory.
- `.npmignore` follows gitignore rules, as npm documents, and whyignore also
  notes what npm decides regardless: `package.json`, `README` and `LICENSE`
  are always packed; `.git`, `.npmrc`, `node_modules` and lock files are never
  packed; files such as `.DS_Store` or `*.orig` are left out unless
  package.json `"files"` lists them.

`--trace` lists all rules that matched during evaluation — including the ones
that were later overridden — which makes `!` re-include puzzles easy to debug:

```console
$ whyignore .gitignore keep.log --trace
keep.log  not ignored  re-included by line 4: !keep.log
    .gitignore:1  *.log  (ignore, matched "keep.log")
    .gitignore:4  !keep.log  (re-include, matched "keep.log")  <- winner
```

With `--json`, each result additionally carries a `trace` array of
`{ line, pattern, negated, matched, winner }` objects.

A path is treated as a directory if it ends with `/`, or if it exists as a
directory under `--root` (default: the ignore file's directory). This matters
because patterns like `build/` only match directories.

## Supported syntax

- `*` and `?` (which do not cross `/`), and `[abc]` / `[!abc]` character classes.
- `**`: leading `**/` (any depth), trailing `/**` (everything inside), and
  `a/**/b` (zero or more directories between).
- A leading `/` anchors a pattern to the ignore file's directory; a pattern
  with an internal `/` is also anchored. A bare name matches at any depth.
- A trailing `/` restricts a pattern to directories.
- `!` re-includes a previously excluded path.
- Comments (`#`) and blank lines are skipped; `\#` / `\!` escape a literal
  first character, and `\` escapes any glob metacharacter (e.g. `foo\*bar`
  matches the literal name `foo*bar`).

The decision uses Git's rules: the last matching pattern wins, and — like Git —
a file cannot be re-included if one of its parent directories is excluded.

## Notes

- whyignore reads a single ignore file. Git's full behavior also combines
  `.gitignore` files from every directory level and the global excludes; those
  are out of scope here.

## Programmatic API

```rust
use whyignore::{parse_rules, evaluate};

let rules = parse_rules("build/\n*.log\n!keep.log\n")?;
let decision = evaluate("build/app.log", false, &rules);
assert!(decision.ignored);
println!("{:?}", decision.matched); // the deciding rule
```

`parse_rules` and `evaluate` are pure functions.

## License

[MIT](./LICENSE) (c) printemps.tokyo
