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

fn run_with_env(directory: &Path, arguments: &[&str], environment: &[(&str, &str)]) -> Output {
    Command::new(concord())
        .current_dir(directory)
        .args(arguments)
        .envs(environment.iter().copied())
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
    assert_eq!(report["schemaVersion"], 2);
    assert_eq!(report["mode"], "lint");
    assert_eq!(report["rawSummary"]["exactMatches"], 1);
    assert!(report["comparableSummary"].is_object());
    assert!(report["mappingSummary"].is_object());
    assert!(report["failures"].is_array());
}

#[test]
fn repeated_lint_json_ignores_raw_structured_tool_telemetry() {
    let directory = project_with_tools(&["eslint", "biome"]);
    fs::write(
        directory.path().join("case.ts"),
        "console.log('value');\ndebugger;\n",
    )
    .expect("source");
    let arguments = [
        "compare",
        "lint",
        "--baseline",
        "eslint",
        "--candidate",
        "biome",
        "--output",
        "json",
        "--no-save-report",
        "case.ts",
    ];

    let first = run_with_env(
        directory.path(),
        &arguments,
        &[
            ("CONCORD_FAKE_ESLINT_DELAY_MS", "150"),
            ("CONCORD_FAKE_BIOME_DELAY_MS", "0"),
        ],
    );
    let second = run_with_env(
        directory.path(),
        &arguments,
        &[
            ("CONCORD_FAKE_ESLINT_DELAY_MS", "0"),
            ("CONCORD_FAKE_BIOME_DELAY_MS", "150"),
        ],
    );
    assert_eq!(first.status.code(), Some(1));
    assert_eq!(second.status.code(), Some(1));
    let mut first_report: serde_json::Value =
        serde_json::from_slice(&first.stdout).expect("first report JSON");
    let mut second_report: serde_json::Value =
        serde_json::from_slice(&second.stdout).expect("second report JSON");
    remove_documented_variable_fields(&mut first_report);
    remove_documented_variable_fields(&mut second_report);

    assert_eq!(first_report, second_report);
    for side in ["baseline", "candidate"] {
        assert!(first_report[side].get("stdout").is_none());
        assert!(first_report[side].get("stderr").is_none());
    }
    assert_eq!(
        first_report["baseline"]["diagnostics"]
            .as_array()
            .map(Vec::len),
        Some(1)
    );
    assert_eq!(
        first_report["candidate"]["diagnostics"]
            .as_array()
            .map(Vec::len),
        Some(1)
    );
}

#[test]
fn invalid_linter_output_retains_failure_context() {
    let directory = project_with_tools(&["eslint", "biome"]);
    fs::write(directory.path().join("case.ts"), "// INVALID_BIOME_JSON\n").expect("source");
    let output = run(
        directory.path(),
        &[
            "compare",
            "lint",
            "--baseline",
            "eslint",
            "--candidate",
            "biome",
            "--output",
            "json",
            "--no-save-report",
            "case.ts",
        ],
    );

    assert_eq!(output.status.code(), Some(3));
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8_lossy(&output.stderr);
    for expected in [
        "failed to parse Biome JSON output",
        "command:",
        "exit code: Some(0)",
        "failure stdout: invalid biome stdout",
        "failure stderr: invalid biome stderr",
        "failed to parse Biome structured JSON",
    ] {
        assert!(
            stderr.contains(expected),
            "missing {expected:?} in {stderr}"
        );
    }
}

#[test]
fn failed_linter_execution_retains_raw_output() {
    let directory = project_with_tools(&["eslint", "biome"]);
    fs::write(directory.path().join("case.ts"), "// BIOME_CRASH\n").expect("source");
    let output = run(
        directory.path(),
        &[
            "compare",
            "lint",
            "--baseline",
            "eslint",
            "--candidate",
            "biome",
            "--output",
            "json",
            "--no-save-report",
            "case.ts",
        ],
    );

    assert_eq!(output.status.code(), Some(3));
    let stderr = String::from_utf8_lossy(&output.stderr);
    for expected in [
        "Biome failed",
        "command:",
        "exit code: Some(2)",
        "failure stdout: partial biome stdout",
        "failure stderr: biome crashed after starting",
    ] {
        assert!(
            stderr.contains(expected),
            "missing {expected:?} in {stderr}"
        );
    }
}

#[test]
fn successful_linter_stderr_is_normalized_as_a_warning() {
    let directory = project_with_tools(&["eslint", "biome"]);
    fs::write(
        directory.path().join("case.ts"),
        "// BIOME_SUCCESS_WARNING\n",
    )
    .expect("source");
    let output = run(
        directory.path(),
        &[
            "compare",
            "lint",
            "--baseline",
            "eslint",
            "--candidate",
            "biome",
            "--output",
            "json",
            "--no-save-report",
            "case.ts",
        ],
    );

    assert_eq!(output.status.code(), Some(0));
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).expect("report JSON");
    assert_eq!(
        report["candidate"]["warnings"],
        serde_json::json!(["Biome stderr:\nBiome configuration warning"])
    );
    assert!(report["candidate"].get("stderr").is_none());
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
        "version = 1\n[execution]\ntimeout_seconds = 4\nformatter_jobs = 1\n",
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
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("timed out"));
    assert!(
        stderr.contains("failure stdout: partial stdout before timeout"),
        "{stderr}"
    );
    assert!(
        stderr.contains("failure stderr: partial stderr before timeout"),
        "{stderr}"
    );
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

#[test]
fn plan_help_is_available_for_both_modes() {
    let directory = tempdir().expect("tempdir");
    for arguments in [["plan", "lint", "--help"], ["plan", "format", "--help"]] {
        let output = run(directory.path(), &arguments);
        assert_eq!(output.status.code(), Some(0));
        assert!(String::from_utf8_lossy(&output.stdout).contains("--report-file"));
    }
}

#[test]
fn plan_reports_missing_tools_without_executing_any_tool() {
    let directory = project_with_tools(&["eslint", "biome"]);
    fs::write(directory.path().join("case.ts"), "debugger;\n").expect("source");
    let log = directory.path().join("invocations.log");
    let output = run_with_env(
        directory.path(),
        &[
            "plan",
            "lint",
            "--baseline",
            "eslint",
            "--candidate",
            "biome",
            "--output",
            "json",
            "--no-save-report",
            ".",
        ],
        &[(
            "CONCORD_FAKE_INVOCATION_LOG",
            log.to_str().expect("log path"),
        )],
    );
    assert_eq!(output.status.code(), Some(0));
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).expect("plan JSON");
    assert_eq!(report["schemaVersion"], 2);
    assert_eq!(report["plan"]["comparable"], serde_json::json!(["case.ts"]));
    assert!(!log.exists(), "plan unexpectedly executed a tool");
}

#[test]
fn comparable_profile_ignores_unmapped_diagnostics_but_keeps_raw_data() {
    let directory = project_with_tools(&["eslint", "biome"]);
    fs::write(directory.path().join("case.ts"), "// CUSTOM_BIOME\n").expect("source");
    let common = [
        "compare",
        "lint",
        "--baseline",
        "eslint",
        "--candidate",
        "biome",
        "--output",
        "json",
        "--no-save-report",
        "case.ts",
    ];
    let raw = run(directory.path(), &common);
    assert_eq!(raw.status.code(), Some(1));
    let comparable = run(
        directory.path(),
        &[
            "compare",
            "lint",
            "--baseline",
            "eslint",
            "--candidate",
            "biome",
            "--profile",
            "comparable",
            "--output",
            "json",
            "--no-save-report",
            "case.ts",
        ],
    );
    assert_eq!(comparable.status.code(), Some(0));
    let report: serde_json::Value =
        serde_json::from_slice(&comparable.stdout).expect("comparable JSON");
    assert_eq!(report["profile"], "comparable");
    assert_eq!(report["rawSummary"]["candidateDiagnostics"], 1);
    assert_eq!(report["comparableSummary"]["candidateDiagnostics"], 0);
    assert_eq!(
        report["unmappedCandidate"].as_array().map(Vec::len),
        Some(1)
    );
}

#[test]
fn unsupported_policy_controls_format_exit_codes_and_is_not_failure() {
    let directory = project_with_tools(&["prettier", "oxfmt"]);
    fs::write(directory.path().join("package-lock.json"), "{}\n").expect("source");
    fs::write(
        directory.path().join("concord.toml"),
        "version = 1\n[tools.oxfmt]\nunsupported = [\"**/package-lock.json\"]\n",
    )
    .expect("config");
    for (policy, expected) in [("ignore", 0), ("difference", 1), ("error", 3)] {
        let output = run(
            directory.path(),
            &[
                "compare",
                "format",
                "--baseline",
                "prettier",
                "--candidate",
                "oxfmt",
                "--unsupported-policy",
                policy,
                "--output",
                "json",
                "--no-save-report",
                "package-lock.json",
            ],
        );
        assert_eq!(output.status.code(), Some(expected), "policy {policy}");
        let report: serde_json::Value =
            serde_json::from_slice(&output.stdout).expect("report JSON");
        assert_eq!(report["summary"]["unsupported"], 1);
        assert_eq!(report["summary"]["failed"], 0);
        assert_eq!(report["files"][0]["status"], "unsupported");
    }
}

#[test]
fn skipped_files_do_not_execute_formatter_processes() {
    let directory = project_with_tools(&["prettier", "oxfmt"]);
    fs::write(directory.path().join("case.ts"), "const value = 1;\n").expect("source");
    fs::write(
        directory.path().join("concord.toml"),
        "version = 1\n[tools.oxfmt]\nexclude = [\"**/*.ts\"]\n",
    )
    .expect("config");
    let log = directory.path().join("invocations.log");
    let output = run_with_env(
        directory.path(),
        &[
            "compare",
            "format",
            "--baseline",
            "prettier",
            "--candidate",
            "oxfmt",
            "--output",
            "json",
            "--no-save-report",
            "case.ts",
        ],
        &[(
            "CONCORD_FAKE_INVOCATION_LOG",
            log.to_str().expect("log path"),
        )],
    );
    assert_eq!(output.status.code(), Some(0));
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).expect("report JSON");
    assert_eq!(report["summary"]["skipped"], 1);
    assert!(!log.exists(), "a skipped comparison executed a tool");
}

#[test]
fn unknown_capability_still_executes_tools() {
    let directory = project_with_tools(&["prettier", "oxfmt"]);
    fs::write(directory.path().join("case.foo"), "const value = 1;\n").expect("source");
    fs::write(
        directory.path().join("concord.toml"),
        "version = 1\n[discovery]\ninclude = [\"**/*.foo\"]\nexclude = []\n",
    )
    .expect("config");
    let log = directory.path().join("invocations.log");
    let output = run_with_env(
        directory.path(),
        &[
            "compare",
            "format",
            "--baseline",
            "prettier",
            "--candidate",
            "oxfmt",
            "--no-save-report",
            "case.foo",
        ],
        &[(
            "CONCORD_FAKE_INVOCATION_LOG",
            log.to_str().expect("log path"),
        )],
    );
    assert_eq!(output.status.code(), Some(0));
    let invocations = fs::read_to_string(log).expect("invocation log");
    assert!(invocations.lines().any(|line| line.contains("case.foo")));
}

#[test]
fn known_oxfmt_versioned_capability_is_unsupported_without_format_failure() {
    let directory = project_with_tools(&["prettier", "oxfmt"]);
    fs::write(directory.path().join("package-lock.json"), "{}\n").expect("source");
    let output = run_with_env(
        directory.path(),
        &[
            "compare",
            "format",
            "--baseline",
            "prettier",
            "--candidate",
            "oxfmt",
            "--unsupported-policy",
            "ignore",
            "--output",
            "json",
            "--no-save-report",
            "package-lock.json",
        ],
        &[("CONCORD_FAKE_OXFMT_VERSION", "0.60.0")],
    );
    assert_eq!(output.status.code(), Some(0));
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).expect("report JSON");
    assert_eq!(report["summary"]["unsupported"], 1);
    assert_eq!(report["summary"]["failed"], 0);
}

#[test]
fn unknown_unsupported_error_remains_a_failure_with_context() {
    let directory = project_with_tools(&["prettier", "oxfmt"]);
    fs::write(directory.path().join("case.ts"), "// UNKNOWN_UNSUPPORTED\n").expect("source");
    let output = run(
        directory.path(),
        &[
            "compare",
            "format",
            "--baseline",
            "prettier",
            "--candidate",
            "oxfmt",
            "--output",
            "json",
            "--no-save-report",
            "case.ts",
        ],
    );
    assert_eq!(output.status.code(), Some(3));
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).expect("report JSON");
    assert_eq!(report["summary"]["failed"], 1);
    assert_eq!(report["summary"]["unsupported"], 0);
    assert!(
        report["files"][0]["candidate"]["stderr"]
            .as_str()
            .is_some_and(|value| value.contains("unsupported internal state"))
    );
}

#[test]
fn formatter_crash_preserves_stdout_and_stderr() {
    let directory = project_with_tools(&["prettier", "oxfmt"]);
    fs::write(directory.path().join("case.ts"), "// FORMAT_CRASH\n").expect("source");
    let output = run(
        directory.path(),
        &[
            "compare",
            "format",
            "--baseline",
            "prettier",
            "--candidate",
            "oxfmt",
            "--output",
            "json",
            "--no-save-report",
            "case.ts",
        ],
    );
    assert_eq!(output.status.code(), Some(3));
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).expect("report JSON");
    for side in ["baseline", "candidate"] {
        assert_eq!(report["files"][0][side]["status"], "failed");
        assert!(
            report["files"][0][side]["output"]
                .as_str()
                .is_some_and(|value| value.contains("partial formatter stdout"))
        );
        assert!(
            report["files"][0][side]["stderr"]
                .as_str()
                .is_some_and(|value| value.contains("formatter crashed"))
        );
    }
    assert_eq!(report["failures"].as_array().map(Vec::len), Some(2));
}

#[test]
fn successful_formatter_stderr_becomes_a_warning() {
    let directory = project_with_tools(&["prettier", "oxfmt"]);
    fs::write(
        directory.path().join("case.ts"),
        "// FORMAT_SUCCESS_WARNING\n",
    )
    .expect("source");
    let output = run(
        directory.path(),
        &[
            "compare",
            "format",
            "--baseline",
            "prettier",
            "--candidate",
            "oxfmt",
            "--output",
            "json",
            "--no-save-report",
            "case.ts",
        ],
    );
    assert_eq!(output.status.code(), Some(0));
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).expect("report JSON");
    for side in ["baseline", "candidate"] {
        assert!(report["files"][0][side].get("stderr").is_none());
        assert_eq!(
            report["files"][0][side]["warnings"]
                .as_array()
                .map(Vec::len),
            Some(1),
            "stderr from both idempotency runs should be normalized and deduplicated"
        );
    }
}

#[test]
fn report_file_is_atomic_and_excluded_from_discovery() {
    let directory = project_with_tools(&["eslint", "oxlint"]);
    fs::write(directory.path().join("case.ts"), "debugger;\n").expect("source");
    let report_path = directory.path().join("reports/report with spaces.json");
    fs::create_dir_all(report_path.parent().expect("report parent")).expect("report directory");
    fs::write(&report_path, "{\"old\":true}\n").expect("old report");
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
            "--report-file",
            "reports/report with spaces.json",
            ".",
        ],
    );
    assert_eq!(output.status.code(), Some(0));
    let report: serde_json::Value =
        serde_json::from_slice(&fs::read(&report_path).expect("saved report"))
            .expect("complete report JSON");
    assert_eq!(
        report["plan"]["discovered"].as_array().map(Vec::len),
        Some(1)
    );
    assert_eq!(report["plan"]["discovered"][0]["path"], "case.ts");
    assert!(
        fs::read_dir(report_path.parent().expect("parent"))
            .expect("reports")
            .all(|entry| !entry
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .ends_with(".tmp"))
    );
    assert!(!directory.path().join(".concord/reports").exists());
}

#[test]
fn report_file_conflicts_with_no_save_report() {
    let directory = tempdir().expect("tempdir");
    let output = run(
        directory.path(),
        &[
            "plan",
            "lint",
            "--baseline",
            "eslint",
            "--candidate",
            "biome",
            "--report-file",
            "report.json",
            "--no-save-report",
        ],
    );
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("cannot be used"));
}

fn remove_documented_variable_fields(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Array(values) => {
            for value in values {
                remove_documented_variable_fields(value);
            }
        }
        serde_json::Value::Object(object) => {
            object.remove("durationMs");
            object.remove("timestamp");
            for value in object.values_mut() {
                remove_documented_variable_fields(value);
            }
        }
        _ => {}
    }
}
