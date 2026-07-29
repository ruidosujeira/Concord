pub mod biome_format;
pub mod biome_lint;
pub mod eslint;
pub mod oxfmt;
pub mod oxlint;
pub mod prettier;

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::thread;

use crate::error::{ConcordError, Result, ToolFailure};
use crate::matching::{AliasTable, sort_diagnostics};
use crate::model::{Diagnostic, Tool, ToolRun, normalize_path};
use crate::process::{ProcessOutput, ProcessRunner, ResolvedTool, display_arguments, truncate};
use crate::report::{FormatFileResult, FormatStatus, FormatterOutcome, unified_diff};

pub fn run_linter(
    runner: &ProcessRunner,
    tool: Tool,
    files: &[PathBuf],
    aliases: &AliasTable,
) -> Result<ToolRun> {
    if !tool.is_linter() {
        return Err(ConcordError::usage(format!(
            "{} is not a supported linter",
            tool
        )));
    }
    let resolved = runner.resolve(tool).map_err(ConcordError::from)?;
    let version = runner
        .version(&resolved)
        .unwrap_or_else(|error| format!("unknown ({error})"));
    let arguments = lint_arguments(tool, runner.root(), files);
    let argument_strings = arguments
        .iter()
        .map(|argument| argument.to_string_lossy().into_owned())
        .collect();
    let output = runner
        .run_resolved(&resolved, arguments.clone(), None)
        .map_err(ConcordError::from)?;
    if !matches!(output.exit_code, Some(0 | 1)) {
        return Err(exit_failure(tool, &resolved, &arguments, &output).into());
    }
    let stdout = String::from_utf8(output.stdout.clone()).map_err(|error| {
        ConcordError::from(ToolFailure::InvalidOutput {
            tool: tool.to_string(),
            format: "JSON".into(),
            executable: resolved.executable.clone(),
            arguments: display_arguments(&arguments),
            exit_code: output.exit_code,
            stderr: truncate(&String::from_utf8_lossy(&output.stderr), 4_096),
            detail: format!("stdout is not UTF-8: {error}"),
        })
    })?;
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    if output.exit_code == Some(1) && stdout.trim().is_empty() {
        return Err(exit_failure(tool, &resolved, &arguments, &output).into());
    }
    let parsed = match tool {
        Tool::Eslint => eslint::parse(&stdout, runner.root(), aliases),
        Tool::Biome => biome_lint::parse(&stdout, runner.root(), aliases),
        Tool::Oxlint => oxlint::parse(&stdout, runner.root(), aliases),
        _ => unreachable!("validated linter"),
    }
    .map_err(|detail| {
        ConcordError::from(ToolFailure::InvalidOutput {
            tool: tool.to_string(),
            format: "JSON".into(),
            executable: resolved.executable.clone(),
            arguments: display_arguments(&arguments),
            exit_code: output.exit_code,
            stderr: truncate(&stderr, 4_096),
            detail,
        })
    })?;
    let mut diagnostics = parsed.diagnostics;
    sort_diagnostics(&mut diagnostics);
    Ok(ToolRun {
        tool,
        executable: resolved.executable.to_string_lossy().into_owned(),
        version,
        arguments: argument_strings,
        exit_code: output.exit_code,
        duration_ms: output.duration_ms,
        diagnostics,
        warnings: parsed.warnings,
        stdout,
        stderr,
    })
}

#[derive(Debug)]
pub struct ParsedDiagnostics {
    pub diagnostics: Vec<Diagnostic>,
    pub warnings: Vec<String>,
}

#[derive(Clone)]
pub struct FormatterAdapter {
    runner: ProcessRunner,
    resolved: ResolvedTool,
    version: String,
}

impl FormatterAdapter {
    pub fn resolve(runner: &ProcessRunner, tool: Tool) -> Result<Self> {
        if !tool.is_formatter() {
            return Err(ConcordError::usage(format!(
                "{} is not a supported formatter",
                tool
            )));
        }
        let resolved = runner.resolve(tool).map_err(ConcordError::from)?;
        let version = runner
            .version(&resolved)
            .unwrap_or_else(|error| format!("unknown ({error})"));
        Ok(Self {
            runner: runner.clone(),
            resolved,
            version,
        })
    }

    pub fn tool(&self) -> Tool {
        self.resolved.tool
    }

    pub fn run(&self, path: &Path, input: &[u8], normalize_eol: bool) -> FormatterOutcome {
        let arguments = formatter_arguments(self.tool(), path);
        let argument_strings = arguments
            .iter()
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect();
        let first = self
            .runner
            .run_resolved(&self.resolved, arguments.clone(), Some(input));
        let first = match first {
            Ok(output) if output.exit_code == Some(0) => output,
            Ok(output) => {
                return failed_formatter_outcome(
                    self,
                    argument_strings,
                    output.exit_code,
                    output.duration_ms,
                    &output.stderr,
                    "formatter returned a non-zero exit code",
                );
            }
            Err(error) => {
                return failed_formatter_outcome(
                    self,
                    argument_strings,
                    None,
                    0,
                    &[],
                    &error.to_string(),
                );
            }
        };
        let first_text = match String::from_utf8(first.stdout.clone()) {
            Ok(value) => value,
            Err(error) => {
                return failed_formatter_outcome(
                    self,
                    argument_strings,
                    first.exit_code,
                    first.duration_ms,
                    &first.stderr,
                    &format!("formatter output is not UTF-8: {error}"),
                );
            }
        };
        let second = self.runner.run_resolved(
            &self.resolved,
            arguments.clone(),
            Some(first_text.as_bytes()),
        );
        match second {
            Ok(second) if second.exit_code == Some(0) => match String::from_utf8(second.stdout) {
                Ok(second_text) => FormatterOutcome {
                    tool: self.tool(),
                    executable: self.resolved.executable.to_string_lossy().into_owned(),
                    version: self.version.clone(),
                    arguments: argument_strings,
                    exit_code: first.exit_code,
                    duration_ms: first.duration_ms + second.duration_ms,
                    idempotent: normalized_eol(&first_text, normalize_eol)
                        == normalized_eol(&second_text, normalize_eol),
                    error: None,
                    output: Some(first_text),
                    stderr: String::from_utf8_lossy(&first.stderr).into_owned(),
                },
                Err(error) => failed_formatter_outcome(
                    self,
                    argument_strings,
                    second.exit_code,
                    first.duration_ms + second.duration_ms,
                    &second.stderr,
                    &format!("second formatter output is not UTF-8: {error}"),
                ),
            },
            Ok(second) => failed_formatter_outcome(
                self,
                argument_strings,
                second.exit_code,
                first.duration_ms + second.duration_ms,
                &second.stderr,
                "formatter failed during the idempotency check",
            ),
            Err(error) => failed_formatter_outcome(
                self,
                argument_strings,
                None,
                first.duration_ms,
                &first.stderr,
                &format!("idempotency check failed: {error}"),
            ),
        }
    }
}

pub fn compare_format_file(
    root: &Path,
    path: &Path,
    input: &[u8],
    baseline: &FormatterAdapter,
    candidate: &FormatterAdapter,
    normalize_eol: bool,
) -> FormatFileResult {
    let (baseline_outcome, candidate_outcome) = thread::scope(|scope| {
        let baseline_handle = scope.spawn(|| baseline.run(path, input, normalize_eol));
        let candidate_handle = scope.spawn(|| candidate.run(path, input, normalize_eol));
        let baseline_outcome = baseline_handle
            .join()
            .unwrap_or_else(|_| panic_outcome(baseline));
        let candidate_outcome = candidate_handle
            .join()
            .unwrap_or_else(|_| panic_outcome(candidate));
        (baseline_outcome, candidate_outcome)
    });
    let status = classify_format(&baseline_outcome, &candidate_outcome, normalize_eol);
    let report_path = normalize_path(root, path);
    let diff = if status == FormatStatus::Different {
        match (&baseline_outcome.output, &candidate_outcome.output) {
            (Some(baseline), Some(candidate)) => {
                let baseline = normalized_eol(baseline, normalize_eol);
                let candidate = normalized_eol(candidate, normalize_eol);
                Some(unified_diff(&report_path, &baseline, &candidate))
            }
            _ => None,
        }
    } else {
        None
    };
    FormatFileResult {
        path: report_path,
        status,
        baseline: baseline_outcome,
        candidate: candidate_outcome,
        diff,
    }
}

fn classify_format(
    baseline: &FormatterOutcome,
    candidate: &FormatterOutcome,
    normalize_eol: bool,
) -> FormatStatus {
    match (baseline.error.is_some(), candidate.error.is_some()) {
        (true, true) => return FormatStatus::BothFailed,
        (true, false) => return FormatStatus::BaselineFailed,
        (false, true) => return FormatStatus::CandidateFailed,
        (false, false) => {}
    }
    if !baseline.idempotent {
        return FormatStatus::BaselineNonIdempotent;
    }
    if !candidate.idempotent {
        return FormatStatus::CandidateNonIdempotent;
    }
    match (&baseline.output, &candidate.output) {
        (Some(left), Some(right))
            if normalized_eol(left, normalize_eol) == normalized_eol(right, normalize_eol) =>
        {
            FormatStatus::Identical
        }
        _ => FormatStatus::Different,
    }
}

fn lint_arguments(tool: Tool, root: &Path, files: &[PathBuf]) -> Vec<OsString> {
    let mut arguments: Vec<OsString> = match tool {
        Tool::Eslint => ["--format", "json"]
            .into_iter()
            .map(OsString::from)
            .collect(),
        Tool::Biome => [
            "lint",
            "--reporter=rdjson",
            "--max-diagnostics=none",
            "--diagnostic-level=info",
            "--no-errors-on-unmatched",
        ]
        .into_iter()
        .map(OsString::from)
        .collect(),
        Tool::Oxlint => ["--format", "json"]
            .into_iter()
            .map(OsString::from)
            .collect(),
        _ => Vec::new(),
    };
    arguments.extend(files.iter().map(|path| {
        path.strip_prefix(root)
            .unwrap_or(path)
            .as_os_str()
            .to_owned()
    }));
    arguments
}

fn formatter_arguments(tool: Tool, path: &Path) -> Vec<OsString> {
    match tool {
        Tool::Prettier => prettier::arguments(path),
        Tool::Biome => biome_format::arguments(path),
        Tool::Oxfmt => oxfmt::arguments(path),
        _ => Vec::new(),
    }
}

fn exit_failure(
    tool: Tool,
    resolved: &ResolvedTool,
    arguments: &[OsString],
    output: &ProcessOutput,
) -> ToolFailure {
    ToolFailure::Exit {
        tool: tool.to_string(),
        executable: resolved.executable.display().to_string(),
        arguments: display_arguments(arguments),
        exit_code: output.exit_code,
        stderr: truncate(&String::from_utf8_lossy(&output.stderr), 4_096),
    }
}

fn failed_formatter_outcome(
    adapter: &FormatterAdapter,
    arguments: Vec<String>,
    exit_code: Option<i32>,
    duration_ms: u128,
    stderr: &[u8],
    error: &str,
) -> FormatterOutcome {
    FormatterOutcome {
        tool: adapter.tool(),
        executable: adapter.resolved.executable.to_string_lossy().into_owned(),
        version: adapter.version.clone(),
        arguments,
        exit_code,
        duration_ms,
        idempotent: false,
        error: Some(error.into()),
        output: None,
        stderr: String::from_utf8_lossy(stderr).into_owned(),
    }
}

fn panic_outcome(adapter: &FormatterAdapter) -> FormatterOutcome {
    failed_formatter_outcome(
        adapter,
        Vec::new(),
        None,
        0,
        &[],
        "formatter worker panicked",
    )
}

fn normalized_eol(value: &str, enabled: bool) -> String {
    if enabled {
        value.replace("\r\n", "\n")
    } else {
        value.to_owned()
    }
}
