pub mod json;
pub mod terminal;

use std::path::Path;

use serde::{Deserialize, Serialize};
use similar::TextDiff;

use crate::matching::{DiagnosticMatch, MatchResult};
use crate::model::{Diagnostic, Tool, ToolRun};
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
        baseline: ToolRun,
        candidate: ToolRun,
        result: MatchResult,
        summary: LintSummary,
    ) -> Self {
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
    use super::unified_diff;

    #[test]
    fn creates_unified_diff() {
        let diff = unified_diff("app.js", "const x=1;\n", "const x = 1;\n");
        assert!(diff.contains("--- baseline/app.js"));
        assert!(diff.contains("+++ candidate/app.js"));
        assert!(diff.contains("-const x=1;"));
        assert!(diff.contains("+const x = 1;"));
    }
}
