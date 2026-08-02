pub mod json;
pub mod terminal;

use std::path::Path;

use clap::ValueEnum;
use serde::{Deserialize, Serialize};
use similar::TextDiff;

use crate::capabilities::{
    ComparisonPlan, FileDisposition, PlannedFile, PlannedToolFile, PlannedToolStatus,
};
use crate::matching::{
    DiagnosticMatch, MappingConfidence, MatchKind, MatchResult, sort_diagnostics,
};
use crate::model::{Diagnostic, Tool, ToolRun};
use crate::process::truncate;
use crate::scoring::{ComparableLintSummary, MappingSummary, RawLintSummary};

pub const REPORT_SCHEMA_VERSION: u32 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "lowercase")]
#[clap(rename_all = "lower")]
pub enum ComparisonProfile {
    Raw,
    Comparable,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfiguredMappingSummary {
    pub exact: usize,
    pub approximate: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanRuleMapping {
    pub baseline_tool: Tool,
    pub baseline: String,
    pub candidate_tool: Tool,
    pub candidate: String,
    pub confidence: MappingConfidence,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlanReport {
    pub schema_version: u32,
    pub mode: String,
    pub baseline_tool: Tool,
    pub candidate_tool: Tool,
    pub plan: ComparisonPlan,
    pub configured_rule_mappings: ConfiguredMappingSummary,
    pub rule_mappings: Vec<PlanRuleMapping>,
    pub failures: Vec<String>,
}

impl PlanReport {
    pub fn new(
        mode: &str,
        baseline_tool: Tool,
        candidate_tool: Tool,
        plan: ComparisonPlan,
        mut rule_mappings: Vec<PlanRuleMapping>,
    ) -> Self {
        rule_mappings.sort();
        rule_mappings.dedup();
        let configured_rule_mappings = ConfiguredMappingSummary {
            exact: rule_mappings
                .iter()
                .filter(|mapping| mapping.confidence == MappingConfidence::Exact)
                .count(),
            approximate: rule_mappings
                .iter()
                .filter(|mapping| mapping.confidence == MappingConfidence::Approximate)
                .count(),
        };
        Self {
            schema_version: REPORT_SCHEMA_VERSION,
            mode: mode.into(),
            baseline_tool,
            candidate_tool,
            plan,
            configured_rule_mappings,
            rule_mappings,
            failures: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LintReport {
    pub schema_version: u32,
    pub mode: String,
    pub profile: ComparisonProfile,
    pub project_root: String,
    pub plan: ComparisonPlan,
    pub baseline: ToolRun,
    pub candidate: ToolRun,
    pub raw_summary: RawLintSummary,
    pub comparable_summary: ComparableLintSummary,
    pub mapping_summary: MappingSummary,
    pub matches: Vec<DiagnosticMatch>,
    pub baseline_only: Vec<Diagnostic>,
    pub candidate_only: Vec<Diagnostic>,
    pub unmapped_baseline: Vec<Diagnostic>,
    pub unmapped_candidate: Vec<Diagnostic>,
    pub unsupported_files: Vec<FileDisposition>,
    pub skipped_files: Vec<FileDisposition>,
    pub failures: Vec<String>,
}

impl LintReport {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        root: &Path,
        profile: ComparisonProfile,
        plan: ComparisonPlan,
        mut baseline: ToolRun,
        mut candidate: ToolRun,
        result: MatchResult,
        raw_summary: RawLintSummary,
        comparable_summary: ComparableLintSummary,
        mapping_summary: MappingSummary,
    ) -> Self {
        prepare_successful_lint_run(&mut baseline);
        prepare_successful_lint_run(&mut candidate);
        Self {
            schema_version: REPORT_SCHEMA_VERSION,
            mode: "lint".into(),
            profile,
            project_root: root.to_string_lossy().replace('\\', "/"),
            unsupported_files: plan.unsupported.clone(),
            skipped_files: plan.skipped.clone(),
            plan,
            baseline,
            candidate,
            raw_summary,
            comparable_summary,
            mapping_summary,
            matches: result.matches,
            baseline_only: result.baseline_only,
            candidate_only: result.candidate_only,
            unmapped_baseline: result.unmapped_baseline,
            unmapped_candidate: result.unmapped_candidate,
            failures: Vec::new(),
        }
    }

    pub fn has_differences(
        &self,
        profile: ComparisonProfile,
        count_probable_as_match: bool,
    ) -> bool {
        if !self.baseline_only.is_empty() || !self.candidate_only.is_empty() {
            return true;
        }
        if profile == ComparisonProfile::Raw
            && (!self.unmapped_baseline.is_empty() || !self.unmapped_candidate.is_empty())
        {
            return true;
        }
        self.matches.iter().any(|item| match item.kind {
            MatchKind::ExactMatch | MatchKind::ApproximateRuleMatch => false,
            MatchKind::ProbableMatch => !count_probable_as_match,
            MatchKind::SeverityChanged | MatchKind::RangeChanged | MatchKind::MessageChanged => {
                true
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

pub fn normalized_success_stderr(tool: Tool, stderr: &str) -> Option<String> {
    let normalized_eol = stderr.replace("\r\n", "\n").replace('\r', "\n");
    let lines: Vec<_> = normalized_eol.lines().collect();
    let start = lines.iter().position(|line| !line.trim().is_empty())?;
    let end = lines.iter().rposition(|line| !line.trim().is_empty())?;
    let normalized = lines[start..=end].join("\n");
    Some(format!("{tool} stderr:\n{}", truncate(&normalized, 4_096)))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolFileStatus {
    Success,
    Unsupported,
    Skipped,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FormatComparisonStatus {
    Identical,
    Different,
    BaselineNonIdempotent,
    CandidateNonIdempotent,
    BothNonIdempotent,
    Unsupported,
    Skipped,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FormatterOutcome {
    pub status: ToolFileStatus,
    pub tool: Tool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub executable: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    pub arguments: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u128>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub idempotent: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stderr: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

impl FormatterOutcome {
    fn planned(planned: &PlannedToolFile, counterpart_blocks: bool) -> Self {
        let (status, reason) = match planned.status {
            PlannedToolStatus::Unsupported => (
                ToolFileStatus::Unsupported,
                planned
                    .reason
                    .clone()
                    .or_else(|| Some("unsupported".into())),
            ),
            PlannedToolStatus::Skipped => (
                ToolFileStatus::Skipped,
                planned.reason.clone().or_else(|| Some("skipped".into())),
            ),
            PlannedToolStatus::Supported | PlannedToolStatus::Unknown => (
                ToolFileStatus::Skipped,
                counterpart_blocks
                    .then(|| "not executed because the other side is not comparable".into()),
            ),
        };
        Self {
            status,
            tool: planned.tool,
            executable: None,
            version: None,
            arguments: Vec::new(),
            exit_code: None,
            duration_ms: None,
            idempotent: None,
            reason,
            error: None,
            output: None,
            stderr: None,
            warnings: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FormatFileResult {
    pub path: String,
    pub status: FormatComparisonStatus,
    pub baseline: FormatterOutcome,
    pub candidate: FormatterOutcome,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diff: Option<String>,
}

impl FormatFileResult {
    pub fn from_plan(file: &PlannedFile) -> Self {
        let status = if matches!(file.baseline.status, PlannedToolStatus::Unsupported)
            || matches!(file.candidate.status, PlannedToolStatus::Unsupported)
        {
            FormatComparisonStatus::Unsupported
        } else {
            FormatComparisonStatus::Skipped
        };
        Self {
            path: file.path.clone(),
            status,
            baseline: FormatterOutcome::planned(&file.baseline, true),
            candidate: FormatterOutcome::planned(&file.candidate, true),
            diff: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FormatSummary {
    pub discovered: usize,
    pub compared: usize,
    pub identical: usize,
    pub different: usize,
    pub baseline_non_idempotent: usize,
    pub candidate_non_idempotent: usize,
    pub both_non_idempotent: usize,
    pub unsupported: usize,
    pub skipped: usize,
    pub failed: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FormatReport {
    pub schema_version: u32,
    pub mode: String,
    pub project_root: String,
    pub baseline_tool: Tool,
    pub candidate_tool: Tool,
    pub plan: ComparisonPlan,
    pub summary: FormatSummary,
    pub files: Vec<FormatFileResult>,
    pub failures: Vec<String>,
}

impl FormatReport {
    pub fn new(
        root: &Path,
        baseline_tool: Tool,
        candidate_tool: Tool,
        plan: ComparisonPlan,
        mut files: Vec<FormatFileResult>,
    ) -> Self {
        files.sort_by(|left, right| left.path.cmp(&right.path));
        let count = |status| files.iter().filter(|item| item.status == status).count();
        let summary = FormatSummary {
            discovered: plan.discovered_count(),
            compared: files
                .iter()
                .filter(|item| {
                    matches!(
                        item.status,
                        FormatComparisonStatus::Identical
                            | FormatComparisonStatus::Different
                            | FormatComparisonStatus::BaselineNonIdempotent
                            | FormatComparisonStatus::CandidateNonIdempotent
                            | FormatComparisonStatus::BothNonIdempotent
                            | FormatComparisonStatus::Failed
                    )
                })
                .count(),
            identical: count(FormatComparisonStatus::Identical),
            different: count(FormatComparisonStatus::Different),
            baseline_non_idempotent: count(FormatComparisonStatus::BaselineNonIdempotent),
            candidate_non_idempotent: count(FormatComparisonStatus::CandidateNonIdempotent),
            both_non_idempotent: count(FormatComparisonStatus::BothNonIdempotent),
            unsupported: count(FormatComparisonStatus::Unsupported),
            skipped: count(FormatComparisonStatus::Skipped),
            failed: count(FormatComparisonStatus::Failed),
        };
        let failures = files
            .iter()
            .filter(|file| file.status == FormatComparisonStatus::Failed)
            .flat_map(|file| {
                [("baseline", &file.baseline), ("candidate", &file.candidate)]
                    .into_iter()
                    .filter_map(|(side, outcome)| {
                        outcome.error.as_ref().map(|error| {
                            let mut detail =
                                format!("{} {side} {}: {error}", file.path, outcome.tool);
                            if let Some(stdout) = &outcome.output {
                                detail.push_str(&format!("\nstdout: {stdout}"));
                            }
                            if let Some(stderr) = &outcome.stderr {
                                detail.push_str(&format!("\nstderr: {stderr}"));
                            }
                            detail
                        })
                    })
            })
            .collect();
        Self {
            schema_version: REPORT_SCHEMA_VERSION,
            mode: "format".into(),
            project_root: root.to_string_lossy().replace('\\', "/"),
            baseline_tool,
            candidate_tool,
            plan,
            summary,
            files,
            failures,
        }
    }

    pub fn has_differences(&self) -> bool {
        self.summary.different > 0
            || self.summary.baseline_non_idempotent > 0
            || self.summary.candidate_non_idempotent > 0
            || self.summary.both_non_idempotent > 0
    }

    pub fn has_failures(&self) -> bool {
        self.summary.failed > 0
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
    use super::{REPORT_SCHEMA_VERSION, unified_diff};

    #[test]
    fn creates_unified_diff() {
        let diff = unified_diff("app.js", "const x=1;\n", "const x = 1;\n");
        assert!(diff.contains("--- baseline/app.js"));
        assert!(diff.contains("+++ candidate/app.js"));
    }

    #[test]
    fn schema_fixtures_are_explicit_and_distinct() {
        let v1: serde_json::Value =
            serde_json::from_str(include_str!("../../tests/fixtures/schema-v1-lint.json"))
                .expect("schema 1 fixture");
        let v2: serde_json::Value =
            serde_json::from_str(include_str!("../../tests/fixtures/schema-v2-lint.json"))
                .expect("schema 2 fixture");
        assert_eq!(v1["schemaVersion"], 1);
        assert_eq!(v2["schemaVersion"], REPORT_SCHEMA_VERSION);
        assert!(v1.get("rawSummary").is_none());
        assert!(v2.get("rawSummary").is_some());
    }
}
