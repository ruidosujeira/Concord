#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_root="$(cd "$script_dir/.." && pwd)"
smoke_dir="$(mktemp -d)"
trap 'rm -rf "$smoke_dir"' EXIT

cd "$smoke_dir"
npm init --yes >/dev/null
npm install --save-dev --save-exact \
  eslint@10.7.0 \
  @biomejs/biome@2.5.5 \
  oxlint@1.75.0 \
  prettier@3.9.6 \
  oxfmt@0.60.0

mkdir src reports
cat >eslint.config.mjs <<'EOF'
export default [{
  files: ["**/*.{js,ts}"],
  rules: { "no-debugger": "error", "no-console": "warn" },
}];
EOF
cat >biome.json <<'EOF'
{
  "linter": {
    "enabled": true,
    "rules": {
      "recommended": false,
      "suspicious": { "noDebugger": "error", "noConsole": "warn" }
    }
  }
}
EOF
cat >concord.toml <<'EOF'
version = 1

[[matching.rules]]
baseline_tool = "eslint"
baseline = "no-debugger"
candidate_tool = "biome"
candidate = "lint/suspicious/noDebugger"
confidence = "exact"

[[matching.rules]]
baseline_tool = "eslint"
baseline = "no-console"
candidate_tool = "biome"
candidate = "lint/suspicious/noConsole"
confidence = "approximate"
EOF
cat >src/case.js <<'EOF'
const value={answer:42}
console.log(value)
debugger
// DIFF
EOF

run_comparison() {
  set +e
  cargo run --quiet --manifest-path "$repo_root/Cargo.toml" -- "$@"
  status=$?
  set -e
  if [[ "$status" -gt 1 ]]; then
    return "$status"
  fi
}

expect_status() {
  expected="$1"
  shift
  set +e
  cargo run --quiet --manifest-path "$repo_root/Cargo.toml" -- "$@"
  actual=$?
  set -e
  if [[ "$actual" -ne "$expected" ]]; then
    echo "expected exit $expected, got $actual: concord $*" >&2
    return 1
  fi
}

cargo run --quiet --manifest-path "$repo_root/Cargo.toml" -- doctor
cargo run --quiet --manifest-path "$repo_root/Cargo.toml" -- plan lint --baseline eslint --candidate biome --no-save-report src
cargo run --quiet --manifest-path "$repo_root/Cargo.toml" -- plan format --baseline oxfmt --candidate prettier --no-save-report .
run_comparison compare lint --baseline eslint --candidate biome --profile comparable --no-save-report src
run_comparison compare lint --baseline eslint --candidate oxlint --profile comparable --no-save-report src
run_comparison compare format --baseline prettier --candidate biome --no-save-report src
run_comparison compare format --baseline prettier --candidate oxfmt --unsupported-policy ignore --no-save-report src package-lock.json
expect_status 1 compare format --baseline prettier --candidate oxfmt --unsupported-policy difference --no-save-report package-lock.json
expect_status 3 compare format --baseline prettier --candidate oxfmt --unsupported-policy error --no-save-report package-lock.json

run_comparison compare lint --baseline eslint --candidate biome --profile comparable --report-file "reports/concord report.json" src
cargo run --quiet --manifest-path "$repo_root/Cargo.toml" -- plan lint --baseline eslint --candidate biome --report-file reports/plan-first.json src
cargo run --quiet --manifest-path "$repo_root/Cargo.toml" -- plan lint --baseline eslint --candidate biome --report-file reports/plan-second.json src
cmp reports/plan-first.json reports/plan-second.json

test -s "reports/concord report.json"
test -s reports/plan-first.json
test -s reports/plan-second.json
