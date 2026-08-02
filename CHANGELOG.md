# Changelog

All notable changes to Concord are documented here.

## Unreleased

### Added

- Added capability-aware comparison planning.
- Added `unsupported`, `skipped`, and `failed` as distinct outcomes.
- Added per-tool include, exclude, and unsupported file patterns.
- Added explicit rule mappings with exact and approximate confidence.
- Added raw and comparable lint summaries.
- Added the `concord plan` command.
- Added the `--profile comparable` option.
- Added atomic `--report-file` output.
- Added JSON report schema version 2.

### Changed

- Unmapped lint diagnostics are now reported separately from mapped
  baseline-only and candidate-only diagnostics.
- Approximate rule mappings can no longer produce exact matches.
- Formatter failures no longer include known unsupported files.
- The report schema was upgraded from version 1 to version 2.

### Deprecated

- `matching.aliases` is deprecated in favor of `matching.rules`.

### Compatibility

- Existing `concord.toml` version 1 files remain supported.
- Concord v0.1 report consumers must migrate to schema version 2.

## 0.1.2 - 2026-07-30

### Fixed

- Fixed nondeterministic lint JSON reports caused by volatile telemetry embedded
  in raw structured tool output.
- Prevented successful structured tool runs from leaking raw reporter timing
  data into the stable report.
- Preserved stdout and stderr context for operational failures.
- Added regression coverage for repeated report generation and structured
  output with varying internal timing fields.

### Validation

The issue was reproduced against Biome 2.5.5 and with a minimal controlled
fixture.

Two semantically identical lint executions previously produced different JSON
reports because `candidate.stdout` retained fields such as
`summary.duration` and `scannerDuration`.

After this fix, repeated reports remain equivalent after excluding only the
fields officially documented as variable by Concord.

## 0.1.1 - 2026-07-28

### Fixed

- Fixed Biome diagnostic severity normalization by using structured JSON output
  that preserves severity information.
- Fixed off-by-one line and column positions in Biome diagnostics.
- Fixed reducer target drift, where minimization could preserve another
  diagnostic sharing the same rule instead of the mismatch selected by the user.
- Strengthened reducer mismatch signatures using diagnostic messages and
  severities.
- Corrected repository and homepage URLs in Cargo package metadata.

### Validation

The fixes were validated against the TabNews codebase:

- 364 supported, Git-tracked files were evaluated;
- no duplicate diagnostics, false aliases or incorrect proximity matches were
  found in the manually inspected samples;
- reports remained deterministic after excluding execution-duration fields;
- a real mismatch was reduced from 121 lines and 3,329 bytes to 4 lines and
  85 bytes in 23 attempts;
- the reduced case preserved exactly the selected diagnostic.

### Known limitation

Oxfmt 0.60.0 rejects `package-lock.json` when formatting through stdin with
`--stdin-filepath`. After equivalent configuration was applied, the other 363
supported files produced output identical to Prettier.

## 0.1.0 - 2026-07-28

### Added

- Safe local execution and environment diagnosis for ESLint, Biome, Oxlint,
  Prettier and Oxfmt.
- Normalized diagnostic parsing, rule aliases, deterministic matching and
  explicit agreement metrics.
- Exact formatter comparison, unified diffs, bounded concurrency and
  idempotency checks.
- Schema-versioned JSON and compact terminal reports.
- Cached textual delta debugging for lint and format mismatches.
- Cross-platform tests, CI, documentation and optional real-tool smoke tests.
