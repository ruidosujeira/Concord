# Concord

Concord is an open-source Rust CLI that compares the observed behavior of
JavaScript and TypeScript linters and formatters. It runs existing tools,
normalizes their structured output, matches diagnostics deterministically,
reports useful metrics and can minimize a file that preserves a divergence.

The v0.1 implementation supports:

- linters: ESLint, Biome and Oxlint;
- formatters: Prettier, Biome and Oxfmt;
- terminal and schema-versioned JSON reports;
- textual delta debugging for lint and format mismatches.

Concord never installs JavaScript tools. It prefers executables in the target
project's `node_modules/.bin`, falls back to `PATH`, and accepts explicit
executable paths in `concord.toml`. Commands are executed directly, without a
shell, and source files are never passed to a formatter with `--write`.

## Install

Install the official tagged release:

```bash
cargo install \
  --git https://github.com/ruidosujeira/Concord.git \
  --tag v0.1.1 \
  --locked
```

### Install locally

Rust stable is required. From this repository:

```console
cargo install --path .
concord --help
```

Install the JavaScript tools that you want to compare in the target project.
Concord deliberately does not run `npx` or download them.

## Commands

Create a valid, commented configuration:

```console
concord init
concord doctor
```

Compare lint diagnostics:

```console
concord compare lint \
  --baseline eslint \
  --candidate biome \
  src

concord compare lint \
  --baseline eslint \
  --candidate oxlint \
  --output json \
  .
```

Compare formatter output without changing source files:

```console
concord compare format \
  --baseline prettier \
  --candidate oxfmt \
  src

concord compare format \
  --baseline biome \
  --candidate prettier \
  --normalize-eol \
  .
```

Reduce one selected mismatch (indices are zero-based):

```console
concord reduce \
  --mode lint \
  --baseline eslint \
  --candidate biome \
  --mismatch 0 \
  --output repros/case.reduced.ts \
  path/to/case.ts

concord reduce \
  --mode format \
  --baseline prettier \
  --candidate oxfmt \
  path/to/case.ts
```

Comparisons save JSON under `.concord/reports/` by default. Use
`--no-save-report` to disable that copy. `--output terminal` is the default;
`--output json` keeps stdout machine-readable and writes the saved-report path
to stderr.

## Configuration

All fields have defaults. Unknown fields and unsupported configuration versions
are errors.

```toml
version = 1

[discovery]
include = [
  "**/*.js",
  "**/*.jsx",
  "**/*.ts",
  "**/*.tsx",
]
exclude = ["**/generated/**"]

[execution]
timeout_seconds = 30
formatter_jobs = 4

[tools.eslint]
command = "/project/node_modules/.bin/eslint"

[tools.biome]
command = "/project/node_modules/.bin/biome"

[tools.oxlint]
command = "/project/node_modules/.bin/oxlint"

[tools.prettier]
command = "/project/node_modules/.bin/prettier"

[tools.oxfmt]
command = "/project/node_modules/.bin/oxfmt"

[matching]
count_probable_as_match = false

[[matching.aliases]]
eslint = "no-unused-vars"
biome = "lint/correctness/noUnusedVariables"

[[matching.aliases]]
eslint = "@typescript-eslint/no-unused-vars"
oxlint = "typescript/no-unused-vars"
```

An explicit command is resolved first. Without one, Concord checks
`node_modules/.bin` at the project root and then `PATH`. Paths containing spaces
remain one process argument. Timeouts terminate the complete process group (a
job object on Windows).

Discovery supports `.js`, `.jsx`, `.mjs`, `.cjs`, `.ts`, `.tsx`, `.mts`,
`.cts`, `.json` and `.jsonc`, respects `.gitignore`, and excludes `.git`,
`node_modules`, `target`, `dist`, `build`, `coverage` and `.concord` by default.

## Matching and metrics

Rule names are reduced to kebab case after known namespaces are removed. A small
built-in alias table maps common equivalents, and configuration can extend it.
Diagnostics are first grouped by normalized path and canonical rule, then paired
by position. Exact matching requires equal path, canonical rule, span, severity
and message. Correlated diagnostics with a changed span, severity or message get
their own category. A probable match requires the same file and overlapping
ranges or the same start line when the rule IDs cannot be safely correlated.

Probable matches are not exact matches. The primary metrics are:

```text
baseline coverage   = exact matches / baseline diagnostics
candidate precision = exact matches / candidate diagnostics
exact agreement     = 2 * exact matches /
                      (baseline diagnostics + candidate diagnostics)
probable agreement  = 2 * (exact + probable) /
                      (baseline diagnostics + candidate diagnostics)
```

An empty denominator is reported as 100%, making two empty diagnostic sets an
explicit complete agreement. Percentages have one decimal place in terminal
output. Reports also include baseline-only and candidate-only counts, severity,
range and message changes, and tool failures.

Formatter comparisons send the same bytes to each tool over stdin, select the
parser using the source filepath, and run each formatter a second time on its own
output. `--normalize-eol` equates only CRLF with LF for equality and idempotency;
it does not ignore spaces, trailing newlines or any other differences.

## Architecture

The project is one modular crate:

- `process` and `discovery` resolve tools, enforce timeouts and find files;
- `adapters` parse ESLint, Biome JSON/RDJSON and Oxlint JSON, and invoke
  formatters over stdin;
- `model`, `matching` and `scoring` provide normalized data and deterministic
  comparison;
- `report` renders terminal, JSON and unified diffs;
- `reduce` implements cached, line-oriented delta debugging;
- `cli` wires the commands to those layers and maps exit codes.

JSON reports use `schemaVersion: 1`. Their arrays are sorted independently of
the order in which tools or worker threads return results.

## Real-world validation

Concord is tested with captured fixtures, pinned-tool smoke tests and
real-world projects.

The v0.1.1 validation against TabNews uncovered and fixed two correctness bugs:
Biome diagnostic normalization and reducer target drift.

[Read the TabNews validation report](docs/validation/tabnews-v0.1.1.md).

## Exit codes

| Code | Meaning |
| ---: | --- |
| `0` | Comparison completed with no unexpected difference |
| `1` | Differences were found |
| `2` | Invalid CLI usage or configuration |
| `3` | Missing tool, timeout, crash, invalid output or other operational failure |

`doctor` lists missing optional tools without failing. It returns `3` when a
tool explicitly configured in `concord.toml` cannot be used.

## Development

```console
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
cargo run -- --help
cargo run -- init --help
cargo run -- compare lint --help
cargo run -- compare format --help
cargo run -- reduce --help
cargo run -- doctor
```

Normal tests use captured JSON fixtures and local fake executables, so they do
not install or require JavaScript tooling. `scripts/smoke-js.sh` and
`scripts/smoke-js.ps1` are optional networked smoke tests with exact package
versions.

## v0.1 limitations

- Rule aliases are still incomplete.
- A probable match does not mean semantic equivalence.
- The reducer is textual and line-oriented, not AST-aware.
- Highly specific plugins and configurations can create legitimate
  differences.
- Concord compares observed results; it does not prove mathematical
  equivalence.
- Structured reporter formats can change across major tool releases; invalid
  or unsupported output is reported as an operational failure.

Concord is licensed under the MIT License.
