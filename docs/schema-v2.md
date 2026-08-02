# JSON report schema 2

Every report produced by Concord v0.2.0-alpha.1 has:

```json
{"schemaVersion": 2}
```

Field names are camelCase and arrays are deterministically sorted. Paths in
reports use `/` separators. `durationMs` is intentionally variable; reports do
not contain timestamps or raw successful structured reporter telemetry.

## Lint report

The top-level shape contains `mode`, `profile`, `projectRoot`, `plan`,
`baseline`, `candidate`, `rawSummary`, `comparableSummary`, `mappingSummary`,
`matches`, `baselineOnly`, `candidateOnly`, `unmappedBaseline`,
`unmappedCandidate`, `unsupportedFiles`, `skippedFiles`, and `failures`.

`plan.discovered` records both side decisions for every path.
`plan.comparable` lists paths attempted by both tools. The mapping summary lists
distinct observed baseline/candidate IDs and exact, approximate, and unmapped
sets. Both raw and comparable summaries are present regardless of profile.

A diagnostic match has `kind`, `mappingConfidence`, `baseline`, and
`candidate`. An approximate mapping serializes as
`kind: "approximate_rule_match"` and
`mappingConfidence: "approximate"`.

## Format report

The top-level shape contains `mode`, `projectRoot`, `baselineTool`,
`candidateTool`, `plan`, `summary`, `files`, and `failures`. The minimum summary
counts are:

```json
{
  "discovered": 0,
  "compared": 0,
  "identical": 0,
  "different": 0,
  "unsupported": 0,
  "skipped": 0,
  "failed": 0
}
```

Each file has an overall status and separate `baseline` and `candidate`
outcomes. Tool outcome statuses are `success`, `unsupported`, `skipped`, and
`failed`. Comparison statuses are `identical`, `different`,
`baseline_non_idempotent`, `candidate_non_idempotent`,
`both_non_idempotent`, `unsupported`, `skipped`, and `failed`.

`idempotent` exists only after successful execution. Unsupported outcomes have
a reason. Failure outcomes preserve available error, output, and stderr
context. Successful stderr is normalized into `warnings` instead of exposing a
raw channel.

Representative fixtures live at
`tests/fixtures/schema-v1-lint.json` and
`tests/fixtures/schema-v2-lint.json`.

## Report files

`--report-file PATH` creates parent directories, writes a temporary file in the
same directory, flushes it, and atomically persists it to the destination. It
replaces the default `.concord/reports` save and does not alter stdout. With
`--output json`, the same JSON is also printed to stdout. The exact destination
is excluded from discovery, including when it already exists inside the input
scope.
