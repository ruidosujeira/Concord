# Capabilities and comparison plans

Concord v0.2 classifies every globally discovered path independently for the
baseline and candidate before running a comparison.

- `supported`: the adapter has explicit built-in evidence for the file type.
- `unsupported`: configuration or a conservative versioned adapter rule says
  that the tool cannot handle the path.
- `unknown`: Concord has no reliable answer and therefore tries the tool.
- `skipped`: the user excluded the path or it did not match a non-empty
  per-tool `include` list.
- `failed`: an attempted process timed out, crashed, returned an unrecognized
  error, or produced invalid output.

Unknown is deliberately not an error and is never guessed to be unsupported
from a generic stderr regex. Runtime unsupported recognition lives in the
specific adapter. In this alpha, the built-in versioned exception is Oxfmt
0.60.0 formatting `package-lock.json` through stdin. Other versions remain
unknown for that path rather than inheriting the 0.60.0 limitation.

## Configuration

The configuration schema stays at `version = 1`; all new fields default to
empty lists.

```toml
version = 1

[tools.oxfmt]
command = "oxfmt"
include = ["**/*.js", "**/*.ts", "**/*.json"]
exclude = ["**/generated/**"]
unsupported = ["**/package-lock.json"]

[comparison]
unsupported = "difference"
```

Precedence within a tool is `unsupported`, `exclude`, `include`, then the
built-in adapter decision. Globs are compiled once when the configuration is
loaded; an invalid pattern is a usage/configuration error.

The comparison surface contains paths where both sides are either supported or
unknown. If either side is unsupported or skipped, neither formatter is run for
that path. A completely skipped lint comparison also avoids starting a linter.

`concord plan` performs discovery, classification, executable resolution and
mapping catalog inspection, but does not invoke `--version`, lint, or format:

```console
concord plan lint --baseline eslint --candidate biome src
concord plan format --baseline oxfmt --candidate prettier .
```

Missing executables appear in the plan. They are not presented as executed
failures. Counts use only claims Concord can make reliably: discovered,
comparable, unsupported, and skipped by configuration.

## Unsupported policy

- `ignore`: unsupported paths do not affect the exit code.
- `difference` (default): unsupported paths produce exit code 1.
- `error`: unsupported paths produce exit code 3.

Real failures always produce exit code 3. Skipped paths never change the exit
code. The CLI override is `--unsupported-policy` on compare commands.

Capabilities are intentionally incomplete. Concord does not claim knowledge of
every tool version, extension, parser option, or project configuration.
