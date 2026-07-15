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

Prebuilt binaries will be attached to each [release](https://github.com/printemps-tokyo/whyignore/releases) once a version is tagged.

## Usage

```bash
whyignore <ignore-file> <path...>
whyignore .gitignore build/ src/index.ts node_modules/x
whyignore --root . .gitignore dist            # check dir-ness on disk under --root
whyignore .gitignore docs/guide.md --json
whyignore .gitignore keep.log --trace         # show every rule that matched
```

| Option | Description |
| --- | --- |
| `<ignore-file>` | The `.gitignore` (or ignore-style) file to read |
| `<path...>` | One or more paths to test, relative to the ignore file's directory |
| `--root <dir>` | Base directory used to tell whether a path is a directory on disk |
| `--json` | Output JSON instead of the human-readable format |
| `--trace` | Show every matching rule in evaluation order, not just the winner |

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
