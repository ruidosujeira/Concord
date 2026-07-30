pub mod json;
pub mod terminal;

use std::path::Path;

use serde::{Deserialize, Serialize};
use similar::TextDiff;

use crate::matching::{DiagnosticMatch, MatchResult, sort_diagnostics};
use crate::model::{Diagnostic, Tool, ToolRun};
use crate::process::truncate;
use crate::scoring::LintSummary;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LintReport {
    pub schema_version: u32,
    pub mode: String,
    pub project_root: String,
    pub files: usize,
    pub baseline: ToolRun,
    pub candidate: ToolRun,
    pub summary: LintSummary,
    pub matches: Vec<DiagnosticMatch>,
    pub baseline_only: Vec<Diagnostic>,
    pub candidate_only: Vec<Diagnostic>,
    pub failures: Vec<String>,
}

impl LintReport {
    pub fn new(
        root: &Path,
        files: usize,
        mut baseline: ToolRun,
        mut candidate: ToolRun,
        result: MatchResult,
        summary: LintSummary,
    ) -> Self {
        prepare_successful_lint_run(&mut baseline);
        prepare_successful_lint_run(&mut candidate);
        Self {
            schema_version: 1,
            mode: "lint".into(),
            project_root: root.to_string_lossy().replace('\\', "/"),
            files,
            baseline,
            candidate,
            summary,
            matches: result.matches,
            baseline_only: result.baseline_only,
            candidate_only: result.candidate_only,
            failures: Vec::new(),
        }
    }

    pub fn has_differences(&self, count_probable_as_match: bool) -> bool {
        !self.baseline_only.is_empty()
            || !self.candidate_only.is_empty()
            || self.matches.iter().any(|item| {
                use crate::matching::MatchKind;
                match item.kind {
                    MatchKind::ExactMatch => false,
                    MatchKind::ProbableMatch => !count_probable_as_match,
                    _ => true,
                }
            })
    }
}

fn prepare_successful_lint_run(run: &mut ToolRun) {
    sort_diagnostics(&mut run.diagnostics);
    if let Some(warning) = normalized_success_stderr(run.tool, &run.stderr) {
        run.warnings.push(warning);
    }
    run.warnings.sort();
    run.warnings.dedup();
    run.stdout.clear();
    run.stderr.clear();
}

fn normalized_success_stderr(tool: Tool, stderr: &str) -> Option<String> {
    let normalized_eol = stderr.replace("\r\n", "\n").replace('\r', "\n");
    let lines: Vec<_> = normalized_eol.lines().collect();
    let start = lines.iter().position(|line| !line.trim().is_empty())?;
    let end = lines.iter().rposition(|line| !line.trim().is_empty())?;
    let normalized = lines[start..=end].join("\n");
    Some(format!("{tool} stderr:\n{}", truncate(&normalized, 4_096)))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FormatStatus {
    Identical,
    Different,
    BaselineNonIdempotent,
    CandidateNonIdempotent,
    BaselineFailed,
    CandidateFailed,
    BothFailed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FormatterOutcome {
    pub tool: Tool,
    pub executable: String,
    pub version: String,
    pub arguments: Vec<String>,
    pub exit_code: Option<i32>,
    pub duration_ms: u128,
    pub idempotent: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub stderr: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FormatFileResult {
    pub path: String,
    pub status: FormatStatus,
    pub baseline: FormatterOutcome,
    pub candidate: FormatterOutcome,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diff: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FormatSummary {
    pub files: usize,
    pub identical: usize,
    pub different: usize,
    pub baseline_non_idempotent: usize,
    pub candidate_non_idempotent: usize,
    pub baseline_failed: usize,
    pub candidate_failed: usize,
    pub both_failed: usize,
    pub tool_failures: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FormatReport {
    pub schema_version: u32,
    pub mode: String,
    pub project_root: String,
    pub baseline_tool: Tool,
    pub candidate_tool: Tool,
    pub summary: FormatSummary,
    pub files: Vec<FormatFileResult>,
    pub failures: Vec<String>,
}

impl FormatReport {
    pub fn new(
        root: &Path,
        baseline_tool: Tool,
        candidate_tool: Tool,
        mut files: Vec<FormatFileResult>,
    ) -> Self {
        files.sort_by(|left, right| left.path.cmp(&right.path));
        let count = |status| files.iter().filter(|item| item.status == status).count();
        let summary = FormatSummary {
            files: files.len(),
            identical: count(FormatStatus::Identical),
            different: count(FormatStatus::Different),
            baseline_non_idempotent: count(FormatStatus::BaselineNonIdempotent),
            candidate_non_idempotent: count(FormatStatus::CandidateNonIdempotent),
            baseline_failed: count(FormatStatus::BaselineFailed),
            candidate_failed: count(FormatStatus::CandidateFailed),
            both_failed: count(FormatStatus::BothFailed),
            tool_failures: files
                .iter()
                .filter(|item| {
                    matches!(
                        item.status,
                        FormatStatus::BaselineFailed
                            | FormatStatus::CandidateFailed
                            | FormatStatus::BothFailed
                    )
                })
                .count(),
        };
        Self {
            schema_version: 1,
            mode: "format".into(),
            project_root: root.to_string_lossy().replace('\\', "/"),
            baseline_tool,
            candidate_tool,
            summary,
            files,
            failures: Vec::new(),
        }
    }

    pub fn has_differences(&self) -> bool {
        self.files
            .iter()
            .any(|item| item.status != FormatStatus::Identical)
    }

    pub fn has_failures(&self) -> bool {
        self.summary.tool_failures > 0
    }
}

pub fn unified_diff(path: &str, baseline: &str, candidate: &str) -> String {
    TextDiff::from_lines(baseline, candidate)
        .unified_diff()
        .context_radius(3)
        .header(&format!("baseline/{path}"), &format!("candidate/{path}"))
        .to_string()
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use crate::matching::compare;
    use crate::model::{Diagnostic, DiagnosticData, Severity, Span, Tool, ToolRun};
    use crate::scoring;

    use super::{LintReport, json, unified_diff};

    #[test]
    fn creates_unified_diff() {
        let diff = unified_diff("app.js", "const x=1;\n", "const x = 1;\n");
        assert!(diff.contains("--- baseline/app.js"));
        assert!(diff.contains("+++ candidate/app.js"));
        assert!(diff.contains("-const x=1;"));
        assert!(diff.contains("+const x = 1;"));
    }

    #[test]
    fn successful_lint_report_ignores_raw_biome_timings() {
        let first = lint_report(
            tool_run(Tool::Eslint, "[]", "", Vec::new()),
            tool_run(
                Tool::Biome,
                r#"{"summary":{"duration":12,"scannerDuration":4},"diagnostics":[]}"#,
                "",
                Vec::new(),
            ),
        );
        let second = lint_report(
            tool_run(Tool::Eslint, "[]", "", Vec::new()),
            tool_run(
                Tool::Biome,
                r#"{"summary":{"duration":89,"scannerDuration":31},"diagnostics":[]}"#,
                "",
                Vec::new(),
            ),
        );

        assert_eq!(
            json::render(&first).expect("first report"),
            json::render(&second).expect("second report")
        );
    }

    #[test]
    fn successful_lint_report_omits_arbitrary_raw_output() {
        let first = lint_report(
            tool_run(Tool::Eslint, "eslint raw one", "", Vec::new()),
            tool_run(Tool::Biome, "biome raw one", "\n", Vec::new()),
        );
        let second = lint_report(
            tool_run(Tool::Eslint, "eslint raw two", "  \r\n", Vec::new()),
            tool_run(Tool::Biome, "biome raw two", "", Vec::new()),
        );
        let first_json = json::render(&first).expect("first report");
        let second_json = json::render(&second).expect("second report");

        assert_eq!(first_json, second_json);
        let value: serde_json::Value = serde_json::from_str(&first_json).expect("report JSON");
        for side in ["baseline", "candidate"] {
            assert!(value[side].get("stdout").is_none());
            assert!(value[side].get("stderr").is_none());
        }
    }

    #[test]
    fn successful_lint_report_normalizes_stderr_as_a_warning() {
        let first = lint_report(
            tool_run(Tool::Eslint, "[]", "", Vec::new()),
            tool_run(
                Tool::Biome,
                r#"{"diagnostics":[]}"#,
                "\r\nconfiguration warning\r\n  use the new option\r\n\r\n",
                Vec::new(),
            ),
        );
        let second = lint_report(
            tool_run(Tool::Eslint, "different raw", "\n", Vec::new()),
            tool_run(
                Tool::Biome,
                "different biome raw",
                "configuration warning\n  use the new option",
                Vec::new(),
            ),
        );
        let first_json = json::render(&first).expect("first report");

        assert_eq!(first_json, json::render(&second).expect("second report"));
        let value: serde_json::Value = serde_json::from_str(&first_json).expect("report JSON");
        assert_eq!(
            value["candidate"]["warnings"],
            serde_json::json!(["Biome stderr:\nconfiguration warning\n  use the new option"])
        );
        assert!(value["candidate"].get("stderr").is_none());
    }

    #[test]
    fn lint_report_orders_files_and_keeps_normalized_diagnostics() {
        let baseline = vec![
            diagnostic(Tool::Eslint, "src/z.ts", 9, "no-console"),
            diagnostic(Tool::Eslint, "src/a.ts", 2, "no-debugger"),
        ];
        let candidate = vec![
            diagnostic(Tool::Biome, "src/z.ts", 9, "no-console"),
            diagnostic(Tool::Biome, "src/a.ts", 2, "no-debugger"),
        ];
        let first = lint_report(
            tool_run(Tool::Eslint, "raw", "", baseline.clone()),
            tool_run(Tool::Biome, "raw", "", candidate.clone()),
        );
        let second = lint_report(
            tool_run(
                Tool::Eslint,
                "other raw",
                "",
                baseline.into_iter().rev().collect(),
            ),
            tool_run(
                Tool::Biome,
                "other raw",
                "",
                candidate.into_iter().rev().collect(),
            ),
        );
        let first_json = json::render(&first).expect("first report");

        assert_eq!(first_json, json::render(&second).expect("second report"));
        let value: serde_json::Value = serde_json::from_str(&first_json).expect("report JSON");
        assert_eq!(
            value["baseline"]["diagnostics"].as_array().map(Vec::len),
            Some(2)
        );
        assert_eq!(
            value["baseline"]["diagnostics"][0]["path"],
            serde_json::json!("src/a.ts")
        );
        assert_eq!(value["matches"].as_array().map(Vec::len), Some(2));
    }

    fn lint_report(baseline: ToolRun, candidate: ToolRun) -> LintReport {
        let result = compare(baseline.diagnostics.clone(), candidate.diagnostics.clone());
        let summary = scoring::calculate(&result);
        LintReport::new(
            Path::new("project"),
            2,
            baseline,
            candidate,
            result,
            summary,
        )
    }

    fn tool_run(tool: Tool, stdout: &str, stderr: &str, diagnostics: Vec<Diagnostic>) -> ToolRun {
        ToolRun {
            tool,
            executable: tool.config_key().into(),
            version: "1.0.0".into(),
            arguments: vec!["--format=json".into()],
            exit_code: Some(0),
            duration_ms: 0,
            diagnostics,
            warnings: Vec::new(),
            stdout: stdout.into(),
            stderr: stderr.into(),
        }
    }

    fn diagnostic(tool: Tool, path: &str, line: u32, code: &str) -> Diagnostic {
        Diagnostic::new(
            tool,
            path,
            DiagnosticData {
                code: Some(code.into()),
                canonical_code: Some(code.into()),
                severity: Severity::Warning,
                message: format!("{code} diagnostic"),
                span: Some(Span {
                    start_line: line,
                    start_column: 1,
                    end_line: Some(line),
                    end_column: Some(2),
                }),
                fix: None,
            },
        )
    }
}
