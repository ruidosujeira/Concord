# Concord

Concord is an open-source Rust CLI that compares the observed behavior of
JavaScript and TypeScript linters and formatters. It runs existing tools,
normalizes their structured output, matches diagnostics deterministically,
reports useful metrics and can minimize a file that preserves a divergence.

Concord supports:

- linters: ESLint, Biome and Oxlint;
- formatters: Prettier, Biome and Oxfmt;
- terminal and schema-versioned JSON reports;
- textual delta debugging for lint and format mismatches;
- corpus runs over many pinned projects, with findings deduplicated across
  them and ranked by how many projects each one affects.

Concord never installs JavaScript tools. It prefers executables in the target
project's `node_modules/.bin`, falls back to `PATH`, and accepts explicit
executable paths in `concord.toml`. Commands are executed directly, without a
shell, and source files are never passed to a formatter with `--write`.

## Install

Install the official tagged release:

```bash
cargo install \
  --git https://github.com/ruidosujeira/Concord.git \
  --tag v0.1.2 \
  --locked
```

The repository is currently developing v0.2.0-alpha.1. The stable installation
above remains v0.1.2; the alpha has not been published.

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

Inspect the comparable surface without executing either tool:

```console
concord plan lint --baseline eslint --candidate biome src
concord plan format --baseline oxfmt --candidate prettier .
```

Use only comparable files and mapped rules for the primary lint result:

```console
concord compare lint \
  --baseline eslint \
  --candidate biome \
  --profile comparable \
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

Compare many projects in one pass and rank the divergences by how many of
them are affected:

```console
concord corpus \
  --mode lint \
  --baseline eslint \
  --candidate biome \
  --reduce \
  --json corpus.json \
  corpus.txt
```

Comparisons save JSON under `.concord/reports/` by default. Use
`--no-save-report` to disable that copy. `--output terminal` is the default;
`--output json` keeps stdout machine-readable and writes the saved-report path
to stderr. `--report-file reports/concord.json` atomically writes JSON to an
explicit destination and replaces the default save. It cannot be combined with
`--no-save-report`, and the exact destination is excluded from discovery.

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
config = "/project/eslint.config.js"
include = ["**/*.js", "**/*.ts"]
exclude = ["**/generated/**"]
unsupported = ["**/legacy/**"]

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

[[matching.rules]]
baseline_tool = "eslint"
baseline = "@typescript-eslint/no-unused-vars"
candidate_tool = "biome"
candidate = "lint/correctness/noUnusedVariables"
confidence = "exact"
notes = "Equivalent core intent; options may still affect behavior."

[[matching.rules]]
baseline_tool = "eslint"
baseline = "import-x/no-duplicates"
candidate_tool = "biome"
candidate = "lint/suspicious/noDuplicateImports"
confidence = "approximate"

[comparison]
unsupported = "difference"

# Deprecated, but still loaded as exact pairwise mappings.
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

Every tool also accepts an optional `config` path. Relative paths are resolved
from the project that contains `concord.toml` and passed explicitly to the
native tool. Normal single-project comparisons preserve native configuration
discovery when this field is absent. JavaScript or TypeScript configuration
files can execute code in their tool's runtime, so only point `config` at a
file you trust.

Tool `include` narrows that tool's files, `exclude` produces `skipped`, and
`unsupported` records an explicit unsupported capability. Empty lists preserve
v0.1 behavior. Unknown capability is never converted to unsupported: Concord
tries the tool. `comparison.unsupported` defaults to `difference`; the CLI
override is `--unsupported-policy ignore|difference|error`.

Discovery supports `.js`, `.jsx`, `.mjs`, `.cjs`, `.ts`, `.tsx`, `.mts`,
`.cts`, `.json` and `.jsonc`, respects `.gitignore`, and excludes `.git`,
`node_modules`, `target`, `dist`, `build`, `coverage` and `.concord` by default.

## Matching and metrics

Rule names are reduced to kebab case after known namespaces are removed. A small
built-in catalog maps common equivalents. Explicit `matching.rules` entries are
direction-independent and carry `exact` or `approximate` confidence.
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

## Corpus runs

`concord corpus` runs the same differential comparison over many projects in
one invocation and returns one consolidated report in which findings are
deduplicated across projects, ranked by how many projects they affect, and
accompanied by one reproduction each.

It changes nothing about what counts as a divergence. A corpus finding is
exactly what `compare lint` and `compare format` already report as a
difference, observed in one file of one project.

### Manifest format

Plain UTF-8 text, one entry per line, conventionally `corpus.txt`. Blank lines
are ignored, and so is any line whose first non-whitespace character is `#`.
Leading and trailing whitespace on an entry line is trimmed. Every entry
carries a scheme prefix and is fully pinned.

```text
# The published tarball for that exact version.
npm:lodash@4.17.21
npm:@scope/name@1.2.3-alpha.1

# The repository at that exact commit.
git:https://github.com/acme/repo#0123456789abcdef0123456789abcdef01234567

# A local directory, absolute or relative to this manifest file,
# never to the process working directory.
path:projects/alpha
path:../sibling/project
```

A version range, a dist-tag or an omitted npm version is a parse error, and so
is a branch, tag, short sha or omitted git ref: the manifest must be fully
pinned. Every invalid line in a file is reported together, with its 1-based
line number, the offending text and what was expected. Entries that resolve to
the same project are collapsed with a warning on stderr, which is not an error.

### Flags

| Flag | Default | Meaning |
| --- | --- | --- |
| `<MANIFEST>` | — | Path to the manifest |
| `--mode` | — | `lint` or `format` |
| `--baseline` | — | Reference tool |
| `--candidate` | — | Tool being evaluated |
| `--profile` | `raw` | `comparable` drops divergences over unmapped rules |
| `--normalize-eol` | off | Treat CRLF and LF as equal, in `format` mode |
| `--cache-dir` | platform user cache directory, `concord` subdirectory | Acquired entries and run state |
| `--registry` | `https://registry.npmjs.org` | npm registry |
| `--refresh` | off | Discard and re-acquire cached entries |
| `--jobs` | available parallelism | Entries analyzed concurrently |
| `--timeout` | `300` | Seconds covering one entry's entire analysis |
| `--acquire-timeout` | `120` | Seconds covering one entry's acquisition |
| `--max-files-per-entry` | `2000` | Cap on files analyzed per entry |
| `--reduce` | off | Minimize one representative occurrence per finding group |
| `--reduce-timeout` | `120` | Seconds covering one group's reduction |
| `--reduce-top` | all groups | Reduce only the N groups affecting the most projects |
| `--no-resume` | off | Discard prior state and re-run every entry |
| `--quiet` | off | Suppress progress on stderr |
| `--json` | not written | Path for the machine-readable result |
| `--output` | stdout | Path for the human-readable report |

`--jobs 1` produces a fully sequential run. `--jobs` never affects results:
any two job counts produce byte-identical JSON over the same corpus.

### Acquisition and cache

Entries are acquired before any analysis begins, so a network problem surfaces
immediately rather than halfway through a long run. The cache is
content-addressed by entry identity: npm entries under package name and exact
version, git entries under repository URL and commit sha. A cached entry is
reused with no network access at all. `path:` entries are never copied or
cached; they are read in place and never modified.

npm tarballs are fetched over HTTPS and verified against the `dist.integrity`
value from the registry metadata; a mismatch, an absent value or an algorithm
Concord cannot compute is an acquisition failure rather than an unverified
download. Extraction rejects any member that would land outside the
destination, including absolute paths and `..`, and skips links and devices
outright. Git entries prefer a shallow fetch of the specific commit and fall
back to a full clone plus checkout; submodules are never fetched.

Every acquisition is retried twice on transport errors with exponential
backoff. Integrity failures and 404s are not retried.

Nothing acquired is ever executed. No install script runs, no package manager
is invoked, and no build step happens. Both tool executables are resolved once
against the invocation's own project root and pinned as absolute commands, so
an entry that ships its own `node_modules/.bin` can never be run. Native tool
configuration discovery runs from an isolated directory, never from the
entry. If `tools.<name>.config` is set in the invocation's `concord.toml`, that
trusted file is pinned and passed explicitly; otherwise Concord uses the
tool's no-config mode or a generated inert configuration. A JavaScript or
TypeScript configuration explicitly supplied this way remains executable and
must be trusted.

### Fingerprints

Every finding gets a fingerprint that is independent of project identity, file
path, file name, line and column numbers, and any absolute path. It is the hex
SHA-256 digest of these components joined with U+001E:

1. the literal tag `concord-corpus-finding/1`;
2. the mode, `lint` or `format`;
3. the baseline tool key;
4. the candidate tool key;
5. the divergence category, as the existing comparison types name it:
   `baseline_only`, `candidate_only`, `unmapped_baseline`,
   `unmapped_candidate`, `probable_match`, `severity_changed`,
   `range_changed`, `message_changed` for lint, and `different`,
   `baseline_non_idempotent`, `candidate_non_idempotent`,
   `both_non_idempotent` for format;
6. the rule identity — the canonical rule code — or the empty string when the
   divergence has none;
7. the normalized shape of the disagreement.

The shape is built so that the same structural divergence over different names
or literals collapses to one group:

- **format**: the added and removed lines of the unified diff, with file and
  hunk headers dropped. Each line is reduced to a token stream in which every
  distinct identifier becomes a positional placeholder (`$1`, `$2`, …),
  string and template contents become `<str>`, numbers become `<num>`, and
  runs of whitespace become a single `<sp>` or `<nl>`. Language keywords and
  punctuation survive, so `a + b` and `a+b` remain distinct divergences while
  `total + count` and `amount + size` do not.
- **lint**: the severity of each side. The rule identity already names the
  check, and messages embed per-occurrence identifiers, so they are not part
  of the shape — except when there is no rule identity, or when the category
  is `message_changed` and the message difference *is* the divergence. In
  those cases the messages are folded in after collapsing quoted spans and
  symbol-shaped words to `<name>` and numbers to `<num>`.

Findings sharing a fingerprint form a group. The representative occurrence is
chosen deterministically: smallest source file by byte length, ties broken by
lexicographically smallest project identity, then by lexicographically
smallest relative file path.

### Reduction

`--reduce` runs the existing reducer against exactly one representative
occurrence per group, never against every occurrence. `--reduce-top <n>`
limits it to the n groups affecting the most projects, with ties broken by
fingerprint.

The representative file is copied into a scratch directory and reduced there,
so a `path:` entry is never written to. The project root is still what the
tools run against, exactly as in a single-project run; a tool that writes to
its own working directory does so as it always would.

On timeout or reducer failure the group keeps its unreduced representative and
records that reduction did not complete, with the reason.

### Resume

Run state is written under the cache root after each entry completes, to a
temporary file that is then renamed, so an interruption cannot leave a
truncated state file. It is keyed by a digest of the resolved manifest, the
Concord version, the mode, the tool pair, the profile, `--max-files-per-entry`,
`--normalize-eol`, `--timeout`, `--registry` and the whole resolved
`concord.toml`. Changing any of those starts a fresh run rather than resuming
an incompatible one; changing `--jobs`, `--cache-dir`, `--quiet`, the output
destinations or the reduction flags does not.

Completed entries are skipped on the next invocation and their recorded
results are merged with the newly analyzed ones, so a run resumed after an
interruption produces the same report as an uninterrupted one. Entries that
failed are recorded too, so a resumed run does not silently change its result
by retrying them; `--no-resume` forces a full re-run and `--refresh`
re-acquires the entries.

### JSON schema

`--json <path>` writes `schemaVersion: 1`. Field names are camelCase, matching
every other Concord report. The artifact is deterministic across runs over an
unchanged corpus: no timestamps, no wall-clock durations, no absolute paths,
no host or user names, no nondeterministic iteration order.

```jsonc
{
  "schemaVersion": 1,
  "concordVersion": "0.2.0-alpha.1",
  "configuration": {              // only what affects results
    "mode": "lint",
    "baselineTool": "eslint",
    "candidateTool": "biome",
    "profile": "raw",
    "normalizeEol": false,
    "countProbableAsMatch": false,
    "maxFilesPerEntry": 2000,
    "entryTimeoutSeconds": 300,
    "acquireTimeoutSeconds": 120,
    "registry": "https://registry.npmjs.org",
    "reduce": true,
    "reduceTop": 20,              // present only with --reduce
    "reduceTimeoutSeconds": 120   // present only with --reduce
  },
  "summary": {
    "entriesTotal": 4, "entriesAnalyzed": 3, "entriesFailed": 1,
    "entriesTruncated": 0, "distinctFindings": 2, "totalOccurrences": 3
  },
  "entries": [                    // sorted by project identity
    {
      "project": "path:projects/alpha",   // always the manifest form
      "scheme": "path",
      "status": "analyzed",               // or "failed"
      "filesDiscovered": 12,
      "filesAnalyzed": 12,
      "truncated": false,
      "maxFilesPerEntry": 2000,           // present only when truncated
      "occurrences": 1
    }
  ],
  "entryErrors": [                // sorted by kind, then project identity
    {
      "project": "path:projects/docs-only",
      "kind": "no_analyzable_files",
      "message": "no file matches the configured discovery patterns"
    }
  ],
  "findings": [                   // affected projects desc, occurrences desc,
    {                             // then fingerprint asc
      "fingerprint": "88f35ea8…",
      "baselineTool": "eslint",
      "candidateTool": "biome",
      "category": "baseline_only",
      "rule": "no-unused-vars",   // absent when the divergence has none
      "occurrences": 2,
      "affectedProjects": 2,
      "projects": ["path:projects/alpha", "path:projects/beta"],
      "representative": {
        "project": "path:projects/beta",
        "path": "index.ts",       // relative, forward slashes on every platform
        "fileBytes": 68,
        "detail": "baseline_only (baseline)\n  rule: no-unused-vars\n  …"
      },
      "reduction": {              // present only for a reduced group
        "completed": true,
        "reason": "…",            // present only when it did not complete
        "source": "…",            // the minimal reproduction
        "originalBytes": 68, "reducedBytes": 32, "attempts": 2
      }
    }
  ]
}
```

Entry error kinds are `acquisition`, `unreadable`, `no_analyzable_files`,
`tool_failure`, `not_utf8`, `timeout` and `panic`. Their messages have the
project root replaced by `<project>` and any other absolute path by `<path>`.
A reduced `source` is reproduced verbatim, so a project whose own source
contains an absolute path will contain it there too.

### Corpus exit codes

| Code | Meaning |
| ---: | --- |
| `0` | The run completed and produced no finding |
| `1` | The run completed and produced at least one finding |
| `2` | The run itself failed: unreadable, invalid or empty manifest, unusable cache directory, every entry failed acquisition, or an unwritable output path |
| `3` | The run completed, at least one entry failed, and there is no finding |

An entry-level failure never turns a findings-present run into anything other
than `1`. Progress and diagnostics go to stderr and are never interleaved with
report content; progress is suppressed when stderr is not a terminal or when
`--quiet` is passed.

## Architecture

The project is one modular crate:

- `process` and `discovery` resolve tools, enforce timeouts and find files;
- `adapters` parse ESLint, Biome JSON and Oxlint JSON, and invoke
  formatters over stdin;
- `model`, `matching` and `scoring` provide normalized data and deterministic
  comparison;
- `report` renders terminal, JSON and unified diffs;
- `reduce` implements cached, line-oriented delta debugging;
- `corpus` acquires many pinned projects, runs that pipeline over each of
  them, and fingerprints and groups the findings;
- `cli` wires the commands to those layers and maps exit codes.

JSON reports use `schemaVersion: 2`. Their arrays are sorted independently of
the order in which tools or worker threads return results.

Schema 2 contains the comparison plan, raw and comparable summaries, mapping
coverage, separate unmapped arrays, formatter outcomes for both sides, and
distinct unsupported/skipped/failed categories. See
[`docs/schema-v2.md`](docs/schema-v2.md) and the
[`schema 1 migration guide`](docs/migration-schema-v1-v2.md).

Successful structured tool runs are represented through Concord's normalized
model. Raw stdout and stderr are retained only when they are needed to diagnose
an operational failure. Non-empty stderr from a successful run is normalized
into the tool's `warnings` before the raw channel is omitted.

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
tool explicitly configured in `concord.toml` cannot be used. `corpus` reports
run-level failures with `2` and partial failures with `3`; see
[Corpus exit codes](#corpus-exit-codes).

## Development

```console
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
cargo +1.85.0 check --locked --all-targets --all-features
cargo run -- --help
cargo run -- init --help
cargo run -- compare lint --help
cargo run -- compare format --help
cargo run -- reduce --help
cargo run -- corpus --help
cargo run -- doctor
```

Normal tests use captured JSON fixtures and local fake executables, so they do
not install or require JavaScript tooling. `scripts/smoke-js.sh` and
`scripts/smoke-js.ps1` are optional networked smoke tests with exact package
versions.

## Limitations

- Rule aliases are still incomplete.
- A probable match does not mean semantic equivalence.
- The reducer is textual and line-oriented, not AST-aware.
- Highly specific plugins and configurations can create legitimate
  differences.
- Concord compares observed results; it does not prove mathematical
  equivalence.
- A corpus run analyzes acquired projects as data. It does not install their
  dependencies, so a project's own linter plugins and configuration are not in
  play and its divergences are those of the configuration Concord was invoked
  with.
- Formatter non-idempotency findings carry no textual shape, so they group by
  category and tool pair alone.
- Structured reporter formats can change across major tool releases; invalid
  or unsupported output is reported as an operational failure.

Concord is licensed under the MIT License.
