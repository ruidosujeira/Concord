#![cfg(feature = "test-support")]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use assert_cmd::cargo::cargo_bin;
use predicates::prelude::*;
use tempfile::{TempDir, tempdir};

fn concord() -> PathBuf {
    cargo_bin!("concord").to_path_buf()
}

fn fake_tool() -> PathBuf {
    cargo_bin!("concord-fake-tool").to_path_buf()
}

fn run(directory: &Path, arguments: &[&str]) -> Output {
    Command::new(concord())
        .current_dir(directory)
        .args(arguments)
        .output()
        .expect("run concord")
}

fn project_with_tools(tools: &[&str]) -> TempDir {
    let directory = tempdir().expect("tempdir");
    let bin = directory.path().join("node_modules").join(".bin");
    fs::create_dir_all(&bin).expect("create local bin");
    for tool in tools {
        let destination = if cfg!(windows) {
            bin.join(format!("{tool}.exe"))
        } else {
            bin.join(tool)
        };
        fs::copy(fake_tool(), destination).expect("copy fake tool");
    }
    directory
}

#[test]
fn help_is_available() {
    let directory = tempdir().expect("tempdir");
    let output = run(directory.path(), &["--help"]);
    assert_eq!(output.status.code(), Some(0));
    assert!(String::from_utf8_lossy(&output.stdout).contains("Compare JavaScript"));
}

#[test]
fn init_creates_valid_config_and_refuses_overwrite() {
    let directory = tempdir().expect("tempdir");
    let first = run(directory.path(), &["init"]);
    assert_eq!(first.status.code(), Some(0));
    let config = directory.path().join("concord.toml");
    assert!(config.is_file());
    let source = fs::read_to_string(&config).expect("config source");
    let _: concord::config::Config = toml::from_str(&source).expect("valid config");

    let second = run(directory.path(), &["init"]);
    assert_eq!(second.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&second.stderr).contains("--force"));

    let forced = run(directory.path(), &["init", "--force"]);
    assert_eq!(forced.status.code(), Some(0));
}

#[test]
fn missing_tool_is_actionable_and_operational() {
    let directory = tempdir().expect("tempdir");
    fs::write(
        directory.path().join("concord.toml"),
        r#"
version = 1
[tools.eslint]
command = "/definitely/missing/concord-eslint"
[tools.biome]
command = "/definitely/missing/concord-biome"
"#,
    )
    .expect("config");
    fs::write(directory.path().join("case.ts"), "debugger;\n").expect("source");
    let output = run(
        directory.path(),
        &[
            "compare",
            "lint",
            "--baseline",
            "eslint",
            "--candidate",
            "biome",
            "--no-save-report",
            "case.ts",
        ],
    );
    assert_eq!(output.status.code(), Some(3));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(predicate::str::contains("was not found").eval(&stderr));
    assert!(predicate::str::contains("Checked:").eval(&stderr));
}

#[test]
fn lint_json_is_valid_and_stable() {
    let directory = project_with_tools(&["eslint", "oxlint"]);
    fs::write(directory.path().join("case.ts"), "debugger;\n").expect("source");
    let output = run(
        directory.path(),
        &[
            "compare",
            "lint",
            "--baseline",
            "eslint",
            "--candidate",
            "oxlint",
            "--output",
            "json",
            "--no-save-report",
            "case.ts",
        ],
    );
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("valid report JSON");
    assert_eq!(report["schemaVersion"], 1);
    assert_eq!(report["mode"], "lint");
    assert_eq!(report["summary"]["exactMatches"], 1);
    assert!(report["failures"].is_array());
}

#[test]
fn lint_difference_returns_one_and_saves_report() {
    let directory = project_with_tools(&["eslint", "biome"]);
    fs::write(directory.path().join("case.ts"), "debugger;\n").expect("source");
    let output = run(
        directory.path(),
        &[
            "compare",
            "lint",
            "--baseline",
            "eslint",
            "--candidate",
            "biome",
            "case.ts",
        ],
    );
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stdout).contains("BASELINE ONLY"));
    let reports = directory.path().join(".concord").join("reports");
    assert!(fs::read_dir(reports).expect("reports").next().is_some());
}

#[test]
fn format_comparison_does_not_modify_input() {
    let directory = project_with_tools(&["prettier", "oxfmt"]);
    let source_path = directory.path().join("case.ts");
    let original = "const marker = 'DIFF';\n";
    fs::write(&source_path, original).expect("source");
    let output = run(
        directory.path(),
        &[
            "compare",
            "format",
            "--baseline",
            "prettier",
            "--candidate",
            "oxfmt",
            "--no-save-report",
            "case.ts",
        ],
    );
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stdout).contains("Different"));
    assert_eq!(fs::read_to_string(source_path).expect("source"), original);
}

#[test]
fn biome_and_prettier_can_agree() {
    let directory = project_with_tools(&["biome", "prettier"]);
    fs::write(directory.path().join("case.ts"), "const value = 1;\n").expect("source");
    let output = run(
        directory.path(),
        &[
            "compare",
            "format",
            "--baseline",
            "biome",
            "--candidate",
            "prettier",
            "--no-save-report",
            "case.ts",
        ],
    );
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn reducer_minimizes_a_formatter_fixture() {
    let directory = project_with_tools(&["prettier", "oxfmt"]);
    let source = include_str!("fixtures/source/reduce.ts");
    let source_path = directory.path().join("reduce.ts");
    fs::write(&source_path, source).expect("source");
    let output = run(
        directory.path(),
        &[
            "reduce",
            "--mode",
            "format",
            "--baseline",
            "prettier",
            "--candidate",
            "oxfmt",
            "--output",
            "result.ts",
            "--no-save-report",
            "reduce.ts",
        ],
    );
    assert_eq!(
        output.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let reduced = fs::read_to_string(directory.path().join("result.ts")).expect("reduced");
    assert!(reduced.contains("DIFF"));
    assert!(reduced.lines().count() < source.lines().count());
    assert_eq!(fs::read_to_string(source_path).expect("original"), source);
}

#[test]
fn lint_reducer_preserves_the_selected_diagnostic_identity() {
    let directory = project_with_tools(&["eslint", "biome"]);
    let source = r#"try {
    execute();
} catch (error) {
    console.log("failed");
}

const userTryingToGet = getUser();
"#;
    let source_path = directory.path().join("drift.ts");
    fs::write(&source_path, source).expect("source");
    let output = run(
        directory.path(),
        &[
            "reduce",
            "--mode",
            "lint",
            "--baseline",
            "eslint",
            "--candidate",
            "biome",
            "--mismatch",
            "0",
            "--output",
            "result.ts",
            "--no-save-report",
            "drift.ts",
        ],
    );
    assert_eq!(
        output.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let reduced = fs::read_to_string(directory.path().join("result.ts")).expect("reduced");
    assert!(reduced.contains("catch (error)"));
    assert!(!reduced.contains("userTryingToGet"));
    assert_eq!(fs::read_to_string(source_path).expect("original"), source);
}

#[test]
fn usage_error_returns_two() {
    let directory = project_with_tools(&["eslint"]);
    fs::write(directory.path().join("case.ts"), "").expect("source");
    let output = run(
        directory.path(),
        &[
            "compare",
            "lint",
            "--baseline",
            "eslint",
            "--candidate",
            "eslint",
            "case.ts",
        ],
    );
    assert_eq!(output.status.code(), Some(2));
}

#[test]
fn doctor_reports_found_and_missing_tools() {
    let directory = project_with_tools(&["eslint"]);
    let output = Command::new(concord())
        .current_dir(directory.path())
        .env("PATH", "")
        .arg("doctor")
        .output()
        .expect("run doctor");
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("ESLint    found"));
    assert!(stdout.contains("Biome     missing"));
    assert!(stdout.contains("Project root"));
}

#[test]
fn timeout_returns_three() {
    let directory = project_with_tools(&["eslint", "biome"]);
    fs::write(
        directory.path().join("concord.toml"),
        "version = 1\n[execution]\ntimeout_seconds = 1\nformatter_jobs = 1\n",
    )
    .expect("config");
    fs::write(directory.path().join("case.ts"), "// SLOW\ndebugger;\n").expect("source");
    let output = run(
        directory.path(),
        &[
            "compare",
            "lint",
            "--baseline",
            "eslint",
            "--candidate",
            "biome",
            "--no-save-report",
            "case.ts",
        ],
    );
    assert_eq!(output.status.code(), Some(3));
    assert!(String::from_utf8_lossy(&output.stderr).contains("timed out"));
}

#[test]
fn executable_and_source_paths_with_spaces_work() {
    let outer = tempdir().expect("tempdir");
    let project = outer.path().join("project with spaces");
    let bin = project.join("node_modules").join(".bin");
    fs::create_dir_all(&bin).expect("local bin");
    for tool in ["eslint", "oxlint"] {
        let destination = if cfg!(windows) {
            bin.join(format!("{tool}.exe"))
        } else {
            bin.join(tool)
        };
        fs::copy(fake_tool(), destination).expect("copy fake tool");
    }
    let source_directory = project.join("source with spaces");
    fs::create_dir_all(&source_directory).expect("source directory");
    fs::write(source_directory.join("case file.ts"), "debugger;\n").expect("source");
    let output = run(
        &project,
        &[
            "compare",
            "lint",
            "--baseline",
            "eslint",
            "--candidate",
            "oxlint",
            "--no-save-report",
            "source with spaces/case file.ts",
        ],
    );
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
