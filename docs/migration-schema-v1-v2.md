# Migrating report consumers from schema 1 to schema 2

Configuration files do not migrate: `concord.toml` remains `version = 1`.
This guide is only for JSON report consumers.

Reject or branch on `schemaVersion` before reading other fields. Concord never
labels a schema 1 payload as schema 2.

## Lint field mapping

| Schema 1 | Schema 2 |
| --- | --- |
| `files` | `plan.discovered.length` |
| `summary` | `rawSummary` and `comparableSummary` |
| `baselineOnly` | mapped-rule baseline-only only |
| `candidateOnly` | mapped-rule candidate-only only |
| no equivalent | `unmappedBaseline`, `unmappedCandidate` |
| no equivalent | `mappingSummary` |
| no equivalent | `unsupportedFiles`, `skippedFiles` |
| no equivalent | `profile`, `plan` |

Schema 1 baseline-only/candidate-only arrays mixed mapped absences and rules for
which no relationship was known. A consumer cannot reconstruct the schema 2
split from an old report; retain the original category or rerun the comparison.

Do not derive exact agreement by treating approximate mappings as exact. Read
`mappingConfidence` and the two summaries explicitly.

## Format field mapping

Schema 1 encoded failures as `baseline_failed`, `candidate_failed`, or
`both_failed` in the comparison status and used non-optional tool fields.
Schema 2 uses the overall `failed` status plus an independent status for each
side. Unsupported and skipped are not failures. `idempotent` is nullable and is
only set for successful runs.

Schema 1 `summary.files` becomes schema 2 `summary.discovered`.
`summary.compared` counts paths on which comparison execution was attempted.

## Suggested compatibility code

```text
switch report.schemaVersion
  1 -> parse with the frozen schema 1 model
  2 -> parse with the schema 2 mode-specific model
  _ -> stop with an unsupported-schema error
```

Use the representative fixtures in `tests/fixtures` in downstream consumer
tests. Durations remain variable in both versions and should not be used as
deterministic identity fields.
