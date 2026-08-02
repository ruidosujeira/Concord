# Raw and comparable profiles

Lint JSON always retains both raw observations and comparable metrics. The
profile chooses the primary terminal presentation and exit-code decision; it
does not delete audit data.

```console
concord compare lint \
  --baseline eslint \
  --candidate biome \
  --profile comparable \
  .
```

`raw` is the default. All observed diagnostics appear in raw counts, and
unmapped diagnostics can make the comparison different. `comparable` limits
the main decision to paths on the comparable file surface and diagnostics from
rules with a known mapping. Unmapped diagnostics remain in `rawSummary`, the
tool runs, and the `unmapped*` arrays, but do not change its exit code.

Definitions and formulas (all values are percentages):

```text
comparable file coverage = comparable files / discovered files
observed rule mapping coverage = observed rule IDs with a known mapping /
                                 all distinct observed rule IDs
exact agreement = 2 * exact matches /
                  (mapped baseline diagnostics + mapped candidate diagnostics)
comparable agreement = 2 * (exact + approximate + probable matches) /
                       (mapped baseline diagnostics + mapped candidate diagnostics)
```

Changed severity, range, or message remains a mapped difference. Mapped
baseline-only and candidate-only diagnostics remain differences. Approximate
rule matches are reported separately and count toward comparable agreement,
but never exact agreement. Their mapping uncertainty alone does not create a
behavioral difference.

Every empty denominator is defined as 100%. This makes two empty observed sets
an explicit vacuous agreement and an empty discovery surface explicit complete
coverage. “Observed rule mapping coverage” refers only to rule IDs actually
seen in this run; Concord does not introspect or claim coverage of enabled
rules.

Unsupported policy is orthogonal to the profile: `ignore`, `difference`, or
`error` still applies. Skipped paths never affect the exit code, and operational
failures always return 3.
