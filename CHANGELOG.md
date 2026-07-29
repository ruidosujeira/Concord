# Changelog

All notable changes to Concord are documented here.

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
