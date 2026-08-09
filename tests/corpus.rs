#![cfg(feature = "test-support")]
//! Corpus integration tests.
//!
//! Every test here uses fixture project directories checked into the
//! repository and the local fake tool. Nothing reaches the network: the one
//! test that exercises a registry failure serves it from a loopback listener
//! the test itself binds.

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use assert_cmd::cargo::cargo_bin;
use tempfile::{TempDir, tempdir};

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/corpus");

fn concord() -> PathBuf {
    cargo_bin!("concord").to_path_buf()
}

fn fake_tool() -> PathBuf {
    cargo_bin!("concord-fake-tool").to_path_buf()
}

/// A working directory holding the fake tools, copies of the named fixture
/// projects under `projects/`, and a cache directory.
fn workspace(tools: &[&str], fixtures: &[&str]) -> TempDir {
    let directory = tempdir().expect("tempdir");
    let bin = directory.path().join("node_modules").join(".bin");
    fs::create_dir_all(&bin).expect("local bin");
    for tool in tools {
        let destination = if cfg!(windows) {
            bin.join(format!("{tool}.exe"))
        } else {
            bin.join(tool)
        };
        fs::copy(fake_tool(), destination).expect("copy fake tool");
    }
    for fixture in fixtures {
        copy_tree(
            &Path::new(FIXTURES).join(fixture),
            &directory.path().join("projects").join(fixture),
        );
    }
    directory
}

fn copy_tree(source: &Path, destination: &Path) {
    fs::create_dir_all(destination).expect("create destination");
    for entry in fs::read_dir(source).expect("read fixture") {
        let entry = entry.expect("fixture entry");
        let target = destination.join(entry.file_name());
        if entry.file_type().expect("file type").is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).expect("copy fixture file");
        }
    }
}

fn manifest(directory: &Path, name: &str, contents: &str) -> String {
    fs::write(directory.join(name), contents).expect("write manifest");
    name.to_owned()
}

fn corpus_command(directory: &Path, arguments: &[&str]) -> Command {
    let mut command = Command::new(concord());
    command
        .current_dir(directory)
        .arg("corpus")
        .arg("--cache-dir")
        .arg("cache")
        .args(arguments);
    command
}

fn run(directory: &Path, arguments: &[&str]) -> Output {
    corpus_command(directory, arguments)
        .output()
        .expect("run concord corpus")
}

const LINT_PAIR: [&str; 6] = [
    "--mode",
    "lint",
    "--baseline",
    "eslint",
    "--candidate",
    "biome",
];

fn lint_arguments<'a>(arguments: &[&'a str]) -> Vec<&'a str> {
    let mut all = LINT_PAIR.to_vec();
    all.extend_from_slice(arguments);
    all
}

fn lint_run(directory: &Path, arguments: &[&str]) -> Output {
    run(directory, &lint_arguments(arguments))
}

fn lint_run_with_env(directory: &Path, arguments: &[&str], environment: &[(&str, &str)]) -> Output {
    corpus_command(directory, &lint_arguments(arguments))
        .envs(environment.iter().copied())
        .output()
        .expect("run concord corpus")
}

fn json_at(directory: &Path, name: &str) -> serde_json::Value {
    serde_json::from_slice(&fs::read(directory.join(name)).expect("artifact")).expect("valid JSON")
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn help_documents_every_flag() {
    let directory = tempdir().expect("tempdir");
    let output = Command::new(concord())
        .current_dir(directory.path())
        .args(["corpus", "--help"])
        .output()
        .expect("corpus --help");
    assert_eq!(output.status.code(), Some(0));
    let help = stdout(&output);
    for flag in [
        "--mode",
        "--baseline",
        "--candidate",
        "--profile",
        "--normalize-eol",
        "--cache-dir",
        "--registry",
        "--refresh",
        "--jobs",
        "--timeout",
        "--acquire-timeout",
        "--max-files-per-entry",
        "--reduce",
        "--reduce-timeout",
        "--reduce-top",
        "--no-resume",
        "--quiet",
        "--json",
        "--output",
        "<MANIFEST>",
    ] {
        assert!(help.contains(flag), "corpus --help omits {flag}\n{help}");
    }
    for default in ["300", "120", "2000", "https://registry.npmjs.org"] {
        assert!(
            help.contains(default),
            "corpus --help omits the default {default}\n{help}"
        );
    }
}

#[test]
fn one_pass_collapses_shared_divergences_and_separates_distinct_ones() {
    let directory = workspace(
        &["eslint", "biome"],
        &["alpha", "beta", "gamma", "docs-only"],
    );
    let path = directory.path();
    let name = manifest(
        path,
        "corpus.txt",
        "# widely used projects\n\n\
         path:projects/alpha\n\
         path:projects/beta\n\
         path:projects/gamma\n\
         path:projects/docs-only\n",
    );
    let output = lint_run(
        path,
        &["--json", "result.json", "--output", "report.txt", &name],
    );
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));

    let report = json_at(path, "result.json");
    assert_eq!(report["schemaVersion"], 1);
    assert_eq!(report["summary"]["distinctFindings"], 2);
    assert_eq!(report["summary"]["totalOccurrences"], 3);

    // alpha and beta hold the same divergence over different identifiers.
    let first = &report["findings"][0];
    assert_eq!(first["category"], "baseline_only");
    assert_eq!(first["rule"], "no-unused-vars");
    assert_eq!(first["affectedProjects"], 2);
    assert_eq!(
        first["projects"],
        serde_json::json!(["path:projects/alpha", "path:projects/beta"])
    );
    // The representative is the smallest file among the occurrences.
    assert_eq!(first["representative"]["path"], "index.ts");

    // gamma holds a genuinely different divergence.
    let second = &report["findings"][1];
    assert_ne!(first["fingerprint"], second["fingerprint"]);
    assert_eq!(second["category"], "candidate_only");
    assert_eq!(second["rule"], "no-console");
    assert_eq!(second["affectedProjects"], 1);

    let human = fs::read_to_string(path.join("report.txt")).expect("human report");
    assert!(human.starts_with("Concord corpus report"));
    assert!(
        human.contains("3 entries analyzed, 1 failed, 2 distinct findings, 3 total occurrences")
    );
    assert!(human.contains("path:projects/alpha"));
    assert!(
        stdout(&output).is_empty(),
        "--output redirects the report away from stdout"
    );
}

#[test]
fn a_failing_entry_is_isolated_and_the_run_continues() {
    let directory = workspace(&["eslint", "biome"], &["alpha", "docs-only"]);
    let path = directory.path();
    let name = manifest(
        path,
        "corpus.txt",
        "path:projects/docs-only\npath:projects/alpha\npath:projects/missing\n",
    );
    let output = lint_run(path, &["--json", "result.json", &name]);
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));

    let report = json_at(path, "result.json");
    assert_eq!(report["summary"]["entriesAnalyzed"], 1);
    assert_eq!(report["summary"]["entriesFailed"], 2);
    assert_eq!(report["summary"]["distinctFindings"], 1);
    let kinds: Vec<&str> = report["entryErrors"]
        .as_array()
        .expect("entry errors")
        .iter()
        .filter_map(|error| error["kind"].as_str())
        .collect();
    assert_eq!(kinds, ["acquisition", "no_analyzable_files"]);
}

#[test]
fn an_entry_exceeding_the_timeout_is_recorded_without_aborting_the_run() {
    let directory = workspace(&["eslint", "biome"], &["slow", "alpha"]);
    let path = directory.path();
    let name = manifest(
        path,
        "corpus.txt",
        "path:projects/slow\npath:projects/alpha\n",
    );
    let output = lint_run(
        path,
        &[
            "--jobs",
            "1",
            "--timeout",
            "5",
            "--json",
            "result.json",
            &name,
        ],
    );
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));

    let report = json_at(path, "result.json");
    let error = &report["entryErrors"][0];
    assert_eq!(error["project"], "path:projects/slow");
    // The fake tool prints partial output and then sleeps for eight seconds.
    // Waiting it out would have produced unparseable output and a tool
    // failure, so a recorded timeout means the entry really was abandoned.
    assert_eq!(error["kind"], "timeout");
    // The other entry still produced its finding.
    assert_eq!(report["summary"]["entriesAnalyzed"], 1);
    assert_eq!(report["summary"]["distinctFindings"], 1);

    // The abandoned entry's process group went with it.
    assert!(
        !children_of(path),
        "the abandoned entry left a child process behind"
    );
}

/// Whether any process spawned from this workspace's tool directory is still
/// running. Matching on the workspace path keeps concurrent tests apart.
#[cfg(unix)]
fn children_of(workspace: &Path) -> bool {
    let marker = workspace.join("node_modules").join(".bin");
    Command::new("pgrep")
        .args(["-f", &marker.to_string_lossy()])
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

/// Windows offers no equally precise check without another dependency, so the
/// assertion is skipped there.
#[cfg(not(unix))]
fn children_of(_workspace: &Path) -> bool {
    false
}

#[test]
fn the_file_cap_flags_the_entry_and_records_the_cap() {
    let directory = workspace(&["eslint", "biome"], &["alpha", "beta"]);
    let path = directory.path();
    fs::write(
        path.join("projects/alpha/extra.ts"),
        "const anotherUnusedOne = 1;\n",
    )
    .expect("second source file");
    let name = manifest(
        path,
        "corpus.txt",
        "path:projects/alpha\npath:projects/beta\n",
    );
    let output = lint_run(
        path,
        &["--max-files-per-entry", "1", "--json", "result.json", &name],
    );
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));

    let report = json_at(path, "result.json");
    let alpha = &report["entries"][0];
    assert_eq!(alpha["project"], "path:projects/alpha");
    assert_eq!(alpha["truncated"], true);
    assert_eq!(alpha["filesDiscovered"], 2);
    assert_eq!(alpha["filesAnalyzed"], 1);
    assert_eq!(alpha["maxFilesPerEntry"], 1);
    assert_eq!(report["entries"][1]["truncated"], false);
    assert!(report["entries"][1].get("maxFilesPerEntry").is_none());
    assert_eq!(report["summary"]["entriesTruncated"], 1);
}

#[test]
fn exit_codes_match_the_specification() {
    let directory = workspace(&["eslint", "biome"], &["alpha", "docs-only"]);
    let path = directory.path();

    // 1: the run completed and produced at least one finding.
    let findings = manifest(path, "findings.txt", "path:projects/alpha\n");
    assert_eq!(
        lint_run(path, &["--json", "a.json", &findings])
            .status
            .code(),
        Some(1)
    );

    // 0: the run completed and produced no finding. The alpha project holds a
    // divergence only for the eslint/biome pair, not for eslint against
    // itself's mapped counterpart oxlint.
    let clean_directory = workspace(&["eslint", "oxlint"], &["gamma"]);
    let clean = manifest(clean_directory.path(), "clean.txt", "path:projects/gamma\n");
    let output = run(
        clean_directory.path(),
        &[
            "--mode",
            "lint",
            "--baseline",
            "eslint",
            "--candidate",
            "oxlint",
            "--json",
            "b.json",
            &clean,
        ],
    );
    assert_eq!(output.status.code(), Some(0), "{}", stderr(&output));

    // 3: the run completed, an entry failed, and there is no finding.
    let partial = manifest(path, "partial.txt", "path:projects/docs-only\n");
    let output = lint_run(path, &["--json", "c.json", &partial]);
    assert_eq!(output.status.code(), Some(3), "{}", stderr(&output));
    assert_eq!(json_at(path, "c.json")["summary"]["entriesFailed"], 1);

    // 2: the run itself failed.
    let invalid = manifest(path, "invalid.txt", "npm:lodash\n");
    assert_eq!(lint_run(path, &[&invalid]).status.code(), Some(2));
    assert_eq!(
        lint_run(path, &["missing-manifest.txt"]).status.code(),
        Some(2)
    );
    let empty = manifest(path, "empty.txt", "# nothing but a comment\n");
    assert_eq!(lint_run(path, &[&empty]).status.code(), Some(2));
}

#[test]
fn a_malformed_manifest_reports_every_bad_line_in_one_pass() {
    let directory = workspace(&["eslint", "biome"], &["alpha"]);
    let path = directory.path();
    let name = manifest(
        path,
        "corpus.txt",
        "path:projects/alpha\n\
         npm:lodash\n\
         npm:lodash@^4.0.0\n\
         git:https://github.com/acme/repo#main\n\
         ftp://example.com/thing\n",
    );
    let output = lint_run(path, &[&name]);
    assert_eq!(output.status.code(), Some(2));
    let message = stderr(&output);
    assert!(message.contains("4 invalid line(s)"), "{message}");
    for line in ["line 2:", "line 3:", "line 4:", "line 5:"] {
        assert!(message.contains(line), "missing {line} in {message}");
    }
    assert!(message.contains("exact version"), "{message}");
    assert!(message.contains("40-character commit sha"), "{message}");
}

#[test]
fn a_non_utf8_manifest_is_reported_rather_than_crashing() {
    let directory = workspace(&["eslint", "biome"], &["alpha"]);
    let path = directory.path();
    fs::write(
        path.join("corpus.txt"),
        [0x70, 0x61, 0x74, 0x68, 0x3a, 0xff, 0xfe],
    )
    .expect("non-UTF-8 manifest");
    let output = lint_run(path, &["corpus.txt"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(
        stderr(&output).contains("not valid UTF-8"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn duplicate_entries_collapse_with_a_warning() {
    let directory = workspace(&["eslint", "biome"], &["alpha"]);
    let path = directory.path();
    let name = manifest(
        path,
        "corpus.txt",
        "path:projects/alpha\npath:./projects/alpha\npath:projects/../projects/alpha\n",
    );
    let output = lint_run(path, &["--json", "result.json", &name]);
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    assert!(
        stderr(&output).contains("duplicate corpus entry collapsed"),
        "{}",
        stderr(&output)
    );
    let report = json_at(path, "result.json");
    assert_eq!(report["summary"]["entriesTotal"], 1);
    assert_eq!(report["findings"][0]["occurrences"], 1);
}

#[test]
fn two_consecutive_runs_and_different_job_counts_agree_byte_for_byte() {
    let directory = workspace(
        &["eslint", "biome"],
        &["alpha", "beta", "gamma", "docs-only"],
    );
    let path = directory.path();
    let name = manifest(
        path,
        "corpus.txt",
        "path:projects/alpha\n\
         path:projects/beta\n\
         path:projects/gamma\n\
         path:projects/docs-only\n",
    );
    let sequential = lint_run(
        path,
        &[
            "--jobs",
            "1",
            "--no-resume",
            "--json",
            "one.json",
            "--output",
            "one.txt",
            &name,
        ],
    );
    assert_eq!(sequential.status.code(), Some(1), "{}", stderr(&sequential));
    let repeated = lint_run(
        path,
        &[
            "--jobs",
            "1",
            "--no-resume",
            "--json",
            "two.json",
            "--output",
            "two.txt",
            &name,
        ],
    );
    assert_eq!(repeated.status.code(), Some(1));
    let parallel = lint_run(
        path,
        &[
            "--jobs",
            "8",
            "--no-resume",
            "--json",
            "eight.json",
            "--output",
            "eight.txt",
            &name,
        ],
    );
    assert_eq!(parallel.status.code(), Some(1));

    let one = fs::read(path.join("one.json")).expect("first artifact");
    assert_eq!(
        one,
        fs::read(path.join("two.json")).expect("second artifact"),
        "two consecutive runs disagree"
    );
    assert_eq!(
        one,
        fs::read(path.join("eight.json")).expect("parallel artifact"),
        "--jobs 1 and --jobs 8 disagree"
    );

    // The artifact carries nothing machine-specific.
    let text = String::from_utf8(one).expect("UTF-8 artifact");
    for forbidden in ["durationMs", "timestamp", "elapsed"] {
        assert!(!text.contains(forbidden), "artifact contains {forbidden}");
    }
    for line in text.lines() {
        let value = line.split_once(": ").map(|(_, value)| value).unwrap_or("");
        assert!(
            !value.trim_matches(['"', ',']).starts_with('/'),
            "artifact contains an absolute path: {line}"
        );
    }
}

#[test]
fn a_killed_run_resumes_to_an_identical_artifact() {
    let directory = workspace(&["eslint", "biome"], &["alpha", "beta", "slow", "gamma"]);
    let path = directory.path();
    let name = manifest(
        path,
        "corpus.txt",
        "path:projects/alpha\n\
         path:projects/beta\n\
         path:projects/slow\n\
         path:projects/gamma\n",
    );
    let arguments = |json: &str| {
        let mut all: Vec<String> = LINT_PAIR.iter().map(|value| (*value).to_owned()).collect();
        all.extend(
            [
                "--jobs",
                "1",
                "--timeout",
                "5",
                "--quiet",
                "--json",
                json,
                "--output",
            ]
            .map(str::to_owned),
        );
        all.push(format!("{json}.txt"));
        all.push(name.clone());
        all
    };
    fn borrowed(values: &[String]) -> Vec<&str> {
        values.iter().map(String::as_str).collect()
    }

    // An uninterrupted reference run.
    let reference = arguments("reference.json");
    let output = run(path, &borrowed(&reference));
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));

    // A run killed while the third entry is still being analyzed.
    fs::remove_dir_all(path.join("cache")).expect("discard the reference run state");
    let resumed = arguments("resumed.json");
    let mut child: Child = corpus_command(path, &borrowed(&resumed))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn an interruptible run");
    assert!(
        wait_for_state(path, 2),
        "the run never recorded the first two entries"
    );
    child.kill().expect("kill the run");
    let _ = child.wait();

    // Re-invoking resumes and reaches the same result.
    let output = run(path, &borrowed(&resumed));
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    assert_eq!(
        fs::read(path.join("reference.json")).expect("reference artifact"),
        fs::read(path.join("resumed.json")).expect("resumed artifact"),
        "a resumed run differs from an uninterrupted one"
    );
}

/// Wait until the run state records at least `entries` completed entries.
fn wait_for_state(path: &Path, entries: usize) -> bool {
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        if let Some(state) = read_state(path) {
            let recorded = state["entries"].as_object().map_or(0, serde_json::Map::len);
            if recorded >= entries {
                return true;
            }
        }
        thread::sleep(Duration::from_millis(50));
    }
    false
}

fn read_state(path: &Path) -> Option<serde_json::Value> {
    let runs = fs::read_dir(path.join("cache").join("runs")).ok()?;
    for entry in runs.flatten() {
        if entry
            .path()
            .extension()
            .is_some_and(|value| value == "json")
        {
            if let Ok(contents) = fs::read(entry.path()) {
                if let Ok(value) = serde_json::from_slice(&contents) {
                    return Some(value);
                }
            }
        }
    }
    None
}

#[test]
fn changing_a_result_affecting_flag_starts_a_fresh_run() {
    let directory = workspace(&["eslint", "biome"], &["alpha", "gamma"]);
    let path = directory.path();
    let name = manifest(
        path,
        "corpus.txt",
        "path:projects/alpha\npath:projects/gamma\n",
    );
    assert_eq!(
        lint_run(path, &["--json", "raw.json", &name]).status.code(),
        Some(1)
    );
    // A different profile is a different run, not a resume of the first.
    let output = lint_run(
        path,
        &[
            "--profile",
            "comparable",
            "--json",
            "comparable.json",
            &name,
        ],
    );
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    assert_eq!(
        fs::read_dir(path.join("cache").join("runs"))
            .expect("run state directory")
            .count(),
        2,
        "a result-affecting flag must not reuse another run's state"
    );
}

#[test]
fn reduction_minimizes_one_representative_per_group() {
    let directory = workspace(&["eslint", "biome"], &["alpha", "beta", "gamma"]);
    let path = directory.path();
    let name = manifest(
        path,
        "corpus.txt",
        "path:projects/alpha\npath:projects/beta\npath:projects/gamma\n",
    );
    let output = lint_run(path, &["--reduce", "--json", "result.json", &name]);
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));

    let report = json_at(path, "result.json");
    let findings = report["findings"].as_array().expect("findings");
    assert_eq!(findings.len(), 2);
    for group in findings {
        let reduction = &group["reduction"];
        assert_eq!(reduction["completed"], true, "{group}");
        let reduced = reduction["reducedBytes"].as_u64().expect("reduced bytes");
        let original = reduction["originalBytes"].as_u64().expect("original bytes");
        assert!(reduced < original, "{group}");
        assert!(reduction["source"].as_str().is_some_and(|s| !s.is_empty()));
    }

    // The representative's project was never modified by Concord.
    assert_eq!(
        fs::read_to_string(path.join("projects/beta/index.ts")).expect("fixture"),
        fs::read_to_string(Path::new(FIXTURES).join("beta/index.ts")).expect("checked-in fixture"),
        "a path: entry must be treated as read-only"
    );
    // The reducer's working copy and its output belong in a scratch directory,
    // never inside the project. What a tool writes to its own working
    // directory is the tool's doing and is not Concord's to prevent.
    let stray: Vec<String> = fs::read_dir(path.join("projects/beta"))
        .expect("project directory")
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.contains("concord-reduce") || name.contains("reduced"))
        .collect();
    assert!(
        stray.is_empty(),
        "reduction left files inside the project: {stray:?}"
    );
}

#[test]
fn reduce_top_limits_reduction_to_the_largest_groups() {
    let directory = workspace(&["eslint", "biome"], &["alpha", "beta", "gamma"]);
    let path = directory.path();
    let name = manifest(
        path,
        "corpus.txt",
        "path:projects/alpha\npath:projects/beta\npath:projects/gamma\n",
    );
    let output = lint_run(
        path,
        &[
            "--reduce",
            "--reduce-top",
            "1",
            "--json",
            "result.json",
            &name,
        ],
    );
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    let report = json_at(path, "result.json");
    assert_eq!(report["findings"][0]["affectedProjects"], 2);
    assert_eq!(report["findings"][0]["reduction"]["completed"], true);
    assert!(
        report["findings"][1].get("reduction").is_none(),
        "a group outside --reduce-top must not be reduced"
    );
}

#[test]
fn a_reduction_that_cannot_finish_keeps_the_unreduced_representative() {
    let directory = workspace(&["eslint", "biome"], &["alpha"]);
    let path = directory.path();
    let name = manifest(path, "corpus.txt", "path:projects/alpha\n");
    // Every linter invocation takes longer than the whole reduction budget,
    // so the search cannot finish inside it.
    let output = lint_run_with_env(
        path,
        &[
            "--reduce",
            "--reduce-timeout",
            "1",
            "--timeout",
            "60",
            "--json",
            "result.json",
            &name,
        ],
        &[("CONCORD_FAKE_ESLINT_DELAY_MS", "1500")],
    );
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    let report = json_at(path, "result.json");
    let reduction = &report["findings"][0]["reduction"];
    assert_eq!(reduction["completed"], false);
    assert!(
        reduction["reason"]
            .as_str()
            .is_some_and(|value| !value.is_empty()),
        "a reduction that did not complete must record why: {reduction}"
    );
    assert!(reduction.get("source").is_none());
    assert_eq!(
        report["findings"][0]["representative"]["path"],
        "src/app.ts"
    );
}

#[test]
fn an_unavailable_npm_version_fails_only_its_own_entry() {
    let directory = workspace(&["eslint", "biome"], &["alpha"]);
    let path = directory.path();
    let registry = NotFoundRegistry::start();
    let name = manifest(
        path,
        "corpus.txt",
        "npm:concord-nonexistent-package@9.9.9\npath:projects/alpha\n",
    );
    let output = lint_run(
        path,
        &[
            "--registry",
            &registry.url,
            "--acquire-timeout",
            "10",
            "--json",
            "result.json",
            &name,
        ],
    );
    assert_eq!(output.status.code(), Some(1), "{}", stderr(&output));
    let report = json_at(path, "result.json");
    assert_eq!(report["summary"]["entriesFailed"], 1);
    assert_eq!(report["summary"]["entriesAnalyzed"], 1);
    let error = &report["entryErrors"][0];
    assert_eq!(error["project"], "npm:concord-nonexistent-package@9.9.9");
    assert_eq!(error["kind"], "acquisition");
    assert!(
        error["message"].as_str().is_some_and(|m| m.contains("404")),
        "{error}"
    );
    assert_eq!(registry.requests(), 1, "a 404 must not be retried");
}

/// A loopback listener that answers every request with 404, standing in for a
/// registry that does not have the requested version.
struct NotFoundRegistry {
    url: String,
    counter: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

impl NotFoundRegistry {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind a loopback listener");
        let address = listener.local_addr().expect("listener address");
        let counter = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let served = std::sync::Arc::clone(&counter);
        thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                served.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let mut reader = BufReader::new(stream.try_clone().expect("clone stream"));
                let mut line = String::new();
                while reader.read_line(&mut line).unwrap_or(0) > 0 {
                    if line == "\r\n" || line == "\n" {
                        break;
                    }
                    line.clear();
                }
                let _ = stream.write_all(
                    b"HTTP/1.1 404 Not Found\r\nContent-Length: 9\r\nConnection: close\r\n\r\nnot found",
                );
                let _ = stream.flush();
            }
        });
        Self {
            url: format!("http://{address}"),
            counter,
        }
    }

    fn requests(&self) -> usize {
        self.counter.load(std::sync::atomic::Ordering::SeqCst)
    }
}
