# Contributing to Concord

Thank you for helping improve Concord. Keep changes focused, preserve
deterministic output, and do not add automatic downloads of external tools.

## Setup

Install Rust stable, clone the repository, then run:

```console
cargo test --all-features
```

JavaScript tools are not needed for the normal test suite. Parser changes
should include a captured, minimal structured-output fixture. Do not include
secrets or machine-specific absolute paths in fixtures.

## Before opening a pull request

```console
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
cargo run -- --help
```

Add tests for behavioral changes and update `CHANGELOG.md` when the user-facing
behavior changes. Run `scripts/smoke-js.sh` (or the PowerShell equivalent) when
changing real tool invocations; it uses the network and installs pinned
packages only in a temporary directory.
