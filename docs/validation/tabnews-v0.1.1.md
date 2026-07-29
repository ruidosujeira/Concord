# TabNews validation for Concord v0.1.1

Concord v0.1.1 was validated against a real-world JavaScript codebase to verify
diagnostic normalization, matching, formatter comparison and mismatch
reduction.

## Environment

- Concord base commit: `73e6e52081f7798d9b921dd8a2d47bdc94f47b93`
- TabNews commit: `b20da115cc29338c2d30b21e0dbba14f6d1f9ace`
- Rust/Cargo: `1.93.1`
- Node/npm: `24.18.0` / `11.16.0`
- ESLint: `9.39.4`
- Biome: `2.5.5`
- Oxlint: `1.75.0`
- Prettier: `3.8.3`
- Oxfmt: `0.60.0`

## Initial scope

The initial evaluation processed 58 files from `pages/`.

| Comparison | Exit | Result |
| --- | ---: | --- |
| ESLint × Biome | 1 | 0 baseline-only, 16 candidate-only |
| ESLint × Oxlint | 0 | No diagnostics |
| Prettier × Biome | 1 | 57 identical, 1 different |
| Prettier × Oxfmt | 0 | 58 identical |

## Expanded validation

- 199 files were processed in the expanded scope.
- 364 supported, Git-tracked files were processed in the complete scope.
- No operational failures occurred with ESLint, Biome or Oxlint.
- Oxfmt 0.60.0 rejected `package-lock.json` through stdin.
- The other 363 supported files were identical to Prettier after configuring
  `sortPackageJson: false`.

## Bugs uncovered

### Biome diagnostic normalization

A Biome warning at `1:7` was previously represented as `info` at `2:8`.

The adapter now uses structured JSON that preserves severity and keeps one-based
coordinates unchanged.

### Reducer target drift

The reducer could replace the selected mismatch with another diagnostic sharing
the same rule.

Mismatch signatures now include messages and severities, preserving the
specific selected target.

## Reducer validation

```text
Original: 121 lines, 3,329 bytes
Reduced: 4 lines, 85 bytes
Attempts: 23
Duration: 64.8 seconds
```

The reduced file was compared again and preserved exactly:

```text
This variable error is unused.
```

## Manual verification

Direct executions of the underlying tools confirmed:

- five candidate-only diagnostics;
- five baseline-only diagnostics;
- four formatter differences;
- no duplicate diagnostics;
- no false rule aliases;
- no incorrect proximity matches.

Reports were deterministic after excluding duration fields.

## Scope

These results describe the tested TabNews commit and tool versions. They do not
prove universal equivalence between the supported tools.
