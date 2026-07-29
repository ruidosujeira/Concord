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
  oxfmt@0.61.0

mkdir src
cat >eslint.config.mjs <<'EOF'
export default [{
  files: ["**/*.{js,ts}"],
  rules: { "no-debugger": "error", "no-console": "warn" },
}];
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

cargo run --quiet --manifest-path "$repo_root/Cargo.toml" -- doctor
run_comparison compare lint --baseline eslint --candidate biome --no-save-report src
run_comparison compare lint --baseline eslint --candidate oxlint --no-save-report src
run_comparison compare format --baseline prettier --candidate oxfmt --no-save-report src
run_comparison compare format --baseline biome --candidate prettier --no-save-report src
