$ErrorActionPreference = "Stop"

$RepoRoot = Split-Path -Parent $PSScriptRoot
$SmokeDir = Join-Path ([System.IO.Path]::GetTempPath()) ("concord-smoke-" + [guid]::NewGuid())
New-Item -ItemType Directory -Path $SmokeDir | Out-Null

function Invoke-Comparison {
    param([string[]]$Arguments)
    & cargo run --quiet --manifest-path "$RepoRoot/Cargo.toml" -- @Arguments
    if ($LASTEXITCODE -gt 1) {
        throw "Concord comparison failed with exit code $LASTEXITCODE"
    }
}

function Assert-ExitCode {
    param([int]$Expected, [string[]]$Arguments)
    & cargo run --quiet --manifest-path "$RepoRoot/Cargo.toml" -- @Arguments
    if ($LASTEXITCODE -ne $Expected) {
        throw "Expected Concord exit $Expected, got $LASTEXITCODE"
    }
}

try {
    Set-Location $SmokeDir
    npm init --yes | Out-Null
    npm install --save-dev --save-exact eslint@10.7.0 '@biomejs/biome@2.5.5' oxlint@1.75.0 prettier@3.9.6 oxfmt@0.60.0
    New-Item -ItemType Directory -Path src | Out-Null
    New-Item -ItemType Directory -Path reports | Out-Null
    @'
export default [{
  files: ["**/*.{js,ts}"],
  rules: { "no-debugger": "error", "no-console": "warn" },
}];
'@ | Set-Content eslint.config.mjs
    @'
{
  "linter": {
    "enabled": true,
    "rules": {
      "recommended": false,
      "suspicious": { "noDebugger": "error", "noConsole": "warn" }
    }
  }
}
'@ | Set-Content biome.json
    @'
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
'@ | Set-Content concord.toml
    @'
const value={answer:42}
console.log(value)
debugger
// DIFF
'@ | Set-Content src/case.js

    & cargo run --quiet --manifest-path "$RepoRoot/Cargo.toml" -- doctor
    & cargo run --quiet --manifest-path "$RepoRoot/Cargo.toml" -- plan lint --baseline eslint --candidate biome --no-save-report src
    & cargo run --quiet --manifest-path "$RepoRoot/Cargo.toml" -- plan format --baseline oxfmt --candidate prettier --no-save-report .
    Invoke-Comparison @("compare", "lint", "--baseline", "eslint", "--candidate", "biome", "--profile", "comparable", "--no-save-report", "src")
    Invoke-Comparison @("compare", "lint", "--baseline", "eslint", "--candidate", "oxlint", "--profile", "comparable", "--no-save-report", "src")
    Invoke-Comparison @("compare", "format", "--baseline", "prettier", "--candidate", "biome", "--no-save-report", "src")
    Invoke-Comparison @("compare", "format", "--baseline", "prettier", "--candidate", "oxfmt", "--unsupported-policy", "ignore", "--no-save-report", "src", "package-lock.json")
    Assert-ExitCode 1 @("compare", "format", "--baseline", "prettier", "--candidate", "oxfmt", "--unsupported-policy", "difference", "--no-save-report", "package-lock.json")
    Assert-ExitCode 3 @("compare", "format", "--baseline", "prettier", "--candidate", "oxfmt", "--unsupported-policy", "error", "--no-save-report", "package-lock.json")
    Invoke-Comparison @("compare", "lint", "--baseline", "eslint", "--candidate", "biome", "--profile", "comparable", "--report-file", "reports/concord report.json", "src")
    & cargo run --quiet --manifest-path "$RepoRoot/Cargo.toml" -- plan lint --baseline eslint --candidate biome --report-file reports/plan-first.json src
    & cargo run --quiet --manifest-path "$RepoRoot/Cargo.toml" -- plan lint --baseline eslint --candidate biome --report-file reports/plan-second.json src
    if ((Get-FileHash reports/plan-first.json).Hash -ne (Get-FileHash reports/plan-second.json).Hash) {
        throw "Repeated plan reports were not deterministic"
    }
}
finally {
    Set-Location $RepoRoot
    Remove-Item -Recurse -Force $SmokeDir
}
