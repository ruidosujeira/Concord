$ErrorActionPreference = "Stop"

$RepoRoot = Split-Path -Parent $PSScriptRoot
$SmokeDir = Join-Path ([System.IO.Path]::GetTempPath()) ("concord-smoke-" + [guid]::NewGuid())
New-Item -ItemType Directory -Path $SmokeDir | Out-Null

try {
    Set-Location $SmokeDir
    npm init --yes | Out-Null
    npm install --save-dev --save-exact eslint@10.7.0 '@biomejs/biome@2.5.5' oxlint@1.75.0 prettier@3.9.6 oxfmt@0.61.0
    New-Item -ItemType Directory -Path src | Out-Null
    @'
export default [{
  files: ["**/*.{js,ts}"],
  rules: { "no-debugger": "error", "no-console": "warn" },
}];
'@ | Set-Content eslint.config.mjs
    @'
const value={answer:42}
console.log(value)
debugger
// DIFF
'@ | Set-Content src/case.js

    cargo run --quiet --manifest-path "$RepoRoot/Cargo.toml" -- doctor
    $Commands = @(
        @("compare", "lint", "--baseline", "eslint", "--candidate", "biome", "--no-save-report", "src"),
        @("compare", "lint", "--baseline", "eslint", "--candidate", "oxlint", "--no-save-report", "src"),
        @("compare", "format", "--baseline", "prettier", "--candidate", "oxfmt", "--no-save-report", "src"),
        @("compare", "format", "--baseline", "biome", "--candidate", "prettier", "--no-save-report", "src")
    )
    foreach ($Command in $Commands) {
        & cargo run --quiet --manifest-path "$RepoRoot/Cargo.toml" -- @Command
        if ($LASTEXITCODE -gt 1) {
            throw "Concord comparison failed with exit code $LASTEXITCODE"
        }
    }
}
finally {
    Set-Location $RepoRoot
    Remove-Item -Recurse -Force $SmokeDir
}
