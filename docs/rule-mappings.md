# Rule mappings

Lint comparisons need a known relationship between rule IDs before a
diagnostic can be part of the comparable-rule surface.

```toml
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
notes = "Options and fix behavior may differ."
```

Mappings are symmetric: the same entry works when baseline and candidate are
reversed. `exact` allows a diagnostic with equal span, severity, and message to
be an exact match. `approximate` always produces
`approximate_rule_match`; it can never increase `exactMatches` or exact
agreement. This records mapping confidence, not a proof of semantic
equivalence.

Concord rejects same-tool mappings, formatter tools, empty codes, duplicate
pairs, conflicting endpoints, and unknown confidence values. Matching remains
deterministic: diagnostics are sorted, paired within a normalized path and
known rule relationship, then ordered again for reporting. Diagnostics without
a code are conservatively unmapped.

`baselineOnly` and `candidateOnly` mean the observed rule has a known mapping,
but the other tool produced no paired diagnostic. `unmappedBaseline` and
`unmappedCandidate` mean no known mapping exists for the observed rule. Concord
does not pair unmapped diagnostics merely because their messages or locations
look similar.

## Alias compatibility

`matching.aliases` remains accepted for existing version 1 configurations. At
load time each alias group becomes deterministic exact pairwise mappings. A
three-tool group yields the three tool pairs. The file is never rewritten, and
the CLI emits exactly:

```text
warning: matching.aliases is deprecated; use matching.rules
```

Aliases are deprecated because they cannot represent confidence or notes.
They will continue to work throughout the v0.2 alpha compatibility period.
