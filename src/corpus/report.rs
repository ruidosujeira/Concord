//! The two corpus artifacts: a deterministic JSON result and a human-readable
//! report.
//!
//! The JSON artifact contains no timestamp, no wall-clock duration, no
//! absolute path, no host or user name, and no nondeterministic iteration
//! order. Every array is sorted by an explicit key, project identities appear
//! in manifest form, and file paths are relative to the project root with
//! forward slashes on every platform.

use std::fmt::Write;

use serde::{Deserialize, Serialize};

use crate::model::Tool;
use crate::report::ComparisonProfile;

use super::entry::{EntryError, EntryErrorKind, EntryStatus};
use super::group::FindingGroup;
use super::reduction::GroupReduction;

pub const CORPUS_SCHEMA_VERSION: u32 = 1;

/// How many affected projects the human report lists before summarizing.
const LISTED_PROJECTS: usize = 10;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CorpusReport {
    pub schema_version: u32,
    pub concord_version: String,
    pub configuration: ReportConfiguration,
    pub summary: ReportSummary,
    /// Sorted by project identity.
    pub entries: Vec<ReportEntry>,
    /// Sorted by error kind, then project identity.
    pub entry_errors: Vec<ReportEntryError>,
    /// Affected project count descending, occurrence count descending, then
    /// fingerprint ascending.
    pub findings: Vec<ReportGroup>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportConfiguration {
    pub mode: String,
    pub baseline_tool: Tool,
    pub candidate_tool: Tool,
    pub profile: ComparisonProfile,
    pub normalize_eol: bool,
    pub count_probable_as_match: bool,
    pub max_files_per_entry: usize,
    pub entry_timeout_seconds: u64,
    pub acquire_timeout_seconds: u64,
    pub registry: String,
    pub reduce: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reduce_top: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reduce_timeout_seconds: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportSummary {
    pub entries_total: usize,
    pub entries_analyzed: usize,
    pub entries_failed: usize,
    pub entries_truncated: usize,
    pub distinct_findings: usize,
    pub total_occurrences: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportEntry {
    pub project: String,
    pub scheme: String,
    pub status: EntryStatus,
    pub files_discovered: usize,
    pub files_analyzed: usize,
    pub truncated: bool,
    /// Recorded only when the cap was reached.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_files_per_entry: Option<usize>,
    pub occurrences: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportEntryError {
    pub project: String,
    pub kind: EntryErrorKind,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportGroup {
    pub fingerprint: String,
    pub baseline_tool: Tool,
    pub candidate_tool: Tool,
    pub category: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rule: Option<String>,
    pub occurrences: usize,
    pub affected_projects: usize,
    /// Sorted project identities in manifest form.
    pub projects: Vec<String>,
    pub representative: ReportRepresentative,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reduction: Option<GroupReduction>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportRepresentative {
    pub project: String,
    pub path: String,
    pub file_bytes: u64,
    pub detail: String,
}

impl CorpusReport {
    pub fn build(
        configuration: ReportConfiguration,
        mut entries: Vec<ReportEntry>,
        mut entry_errors: Vec<ReportEntryError>,
        groups: &[FindingGroup],
        baseline: Tool,
        candidate: Tool,
    ) -> Self {
        entries.sort_by(|left, right| left.project.cmp(&right.project));
        entry_errors.sort_by(|left, right| {
            left.kind
                .cmp(&right.kind)
                .then_with(|| left.project.cmp(&right.project))
        });
        let findings: Vec<ReportGroup> = groups
            .iter()
            .map(|group| ReportGroup {
                fingerprint: group.fingerprint.clone(),
                baseline_tool: baseline,
                candidate_tool: candidate,
                category: group.category.clone(),
                rule: group.rule.clone(),
                occurrences: group.occurrences,
                affected_projects: group.project_count(),
                projects: group.projects.clone(),
                representative: ReportRepresentative {
                    project: group.representative.project.clone(),
                    path: group.representative.path.clone(),
                    file_bytes: group.representative.file_bytes,
                    detail: group.representative.detail.clone(),
                },
                reduction: group.reduction.clone(),
            })
            .collect();
        let summary = ReportSummary {
            entries_total: entries.len(),
            entries_analyzed: entries
                .iter()
                .filter(|entry| entry.status == EntryStatus::Analyzed)
                .count(),
            entries_failed: entries
                .iter()
                .filter(|entry| entry.status == EntryStatus::Failed)
                .count(),
            entries_truncated: entries.iter().filter(|entry| entry.truncated).count(),
            distinct_findings: findings.len(),
            total_occurrences: findings.iter().map(|group| group.occurrences).sum(),
        };
        Self {
            schema_version: CORPUS_SCHEMA_VERSION,
            concord_version: env!("CARGO_PKG_VERSION").to_owned(),
            configuration,
            summary,
            entries,
            entry_errors,
            findings,
        }
    }

    pub fn has_findings(&self) -> bool {
        !self.findings.is_empty()
    }

    pub fn has_failures(&self) -> bool {
        self.summary.entries_failed > 0
    }
}

/// Render the human-readable report. Nothing here goes to stderr, and nothing
/// from stderr is ever interleaved with it.
pub fn render(report: &CorpusReport) -> String {
    let mut output = String::new();
    let _ = writeln!(output, "Concord corpus report\n");
    let _ = writeln!(
        output,
        "{} entries analyzed, {} failed, {} distinct findings, {} total occurrences",
        report.summary.entries_analyzed,
        report.summary.entries_failed,
        report.summary.distinct_findings,
        report.summary.total_occurrences
    );
    let _ = writeln!(
        output,
        "\nMode         {}\nBaseline     {}\nCandidate    {}",
        report.configuration.mode,
        report.configuration.baseline_tool,
        report.configuration.candidate_tool
    );
    if report.summary.entries_truncated > 0 {
        let _ = writeln!(
            output,
            "\n{} entry/entries reached the {}-file cap and were analyzed only in part.",
            report.summary.entries_truncated, report.configuration.max_files_per_entry
        );
    }

    if report.findings.is_empty() {
        let _ = writeln!(output, "\nNo divergence was found.");
    }
    for (index, group) in report.findings.iter().enumerate() {
        render_group(&mut output, index + 1, group);
    }
    render_errors(&mut output, report);
    output
}

fn render_group(output: &mut String, position: usize, group: &ReportGroup) {
    let _ = writeln!(
        output,
        "\n[{position}] {}{}",
        group.category,
        group
            .rule
            .as_ref()
            .map(|rule| format!("  {rule}"))
            .unwrap_or_default()
    );
    let _ = writeln!(
        output,
        "    {} vs {}",
        group.baseline_tool, group.candidate_tool
    );
    let _ = writeln!(
        output,
        "    Affected projects   {} ({} occurrences)",
        group.affected_projects, group.occurrences
    );
    let _ = writeln!(output, "    Fingerprint         {}", group.fingerprint);
    let _ = writeln!(
        output,
        "    Representative      {} {} ({} bytes)",
        group.representative.project, group.representative.path, group.representative.file_bytes
    );
    for line in group.representative.detail.lines() {
        let _ = writeln!(output, "      {line}");
    }
    match &group.reduction {
        Some(reduction) if reduction.completed => {
            let _ = writeln!(
                output,
                "    Minimal reproduction ({} bytes, was {}, {} attempts)",
                reduction.reduced_bytes, reduction.original_bytes, reduction.attempts
            );
            for line in reduction.source.as_deref().unwrap_or_default().lines() {
                let _ = writeln!(output, "      {line}");
            }
        }
        Some(reduction) => {
            let _ = writeln!(
                output,
                "    Reduction did not complete: {}",
                reduction.reason.as_deref().unwrap_or("unknown reason")
            );
        }
        None => {}
    }
    let _ = writeln!(output, "    Projects");
    for project in group.projects.iter().take(LISTED_PROJECTS) {
        let _ = writeln!(output, "      {project}");
    }
    if group.projects.len() > LISTED_PROJECTS {
        let _ = writeln!(
            output,
            "      … and {} more",
            group.projects.len() - LISTED_PROJECTS
        );
    }
}

fn render_errors(output: &mut String, report: &CorpusReport) {
    if report.entry_errors.is_empty() {
        return;
    }
    let _ = writeln!(output, "\nEntry errors");
    let mut current: Option<EntryErrorKind> = None;
    for error in &report.entry_errors {
        if current != Some(error.kind) {
            let count = report
                .entry_errors
                .iter()
                .filter(|item| item.kind == error.kind)
                .count();
            let _ = writeln!(output, "\n  {} ({count})", error.kind.as_str());
            current = Some(error.kind);
        }
        let _ = writeln!(output, "    {}", error.project);
        for line in error.message.lines().take(4) {
            let _ = writeln!(output, "      {line}");
        }
    }
}

pub fn entry_error(project: &str, error: &EntryError) -> ReportEntryError {
    ReportEntryError {
        project: project.to_owned(),
        kind: error.kind,
        message: error.message.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::{CorpusReport, ReportConfiguration, ReportEntry, ReportEntryError, render};
    use crate::corpus::entry::{EntryErrorKind, EntryStatus};
    use crate::corpus::finding::tests::lint_finding;
    use crate::corpus::group::group;
    use crate::model::Tool;
    use crate::report::ComparisonProfile;

    fn configuration() -> ReportConfiguration {
        ReportConfiguration {
            mode: "lint".into(),
            baseline_tool: Tool::Eslint,
            candidate_tool: Tool::Biome,
            profile: ComparisonProfile::Raw,
            normalize_eol: false,
            count_probable_as_match: false,
            max_files_per_entry: 2_000,
            entry_timeout_seconds: 300,
            acquire_timeout_seconds: 120,
            registry: "https://registry.npmjs.org".into(),
            reduce: false,
            reduce_top: None,
            reduce_timeout_seconds: None,
        }
    }

    fn entry(project: &str, status: EntryStatus) -> ReportEntry {
        ReportEntry {
            project: project.into(),
            scheme: "path".into(),
            status,
            files_discovered: 1,
            files_analyzed: 1,
            truncated: false,
            max_files_per_entry: None,
            occurrences: 0,
        }
    }

    fn report() -> CorpusReport {
        let groups = group(vec![
            lint_finding(
                "baseline_only",
                Some("no-debugger"),
                "m",
                "path:b",
                "b.ts",
                20,
            ),
            lint_finding(
                "baseline_only",
                Some("no-debugger"),
                "m",
                "path:a",
                "a.ts",
                10,
            ),
            lint_finding(
                "candidate_only",
                Some("no-console"),
                "m",
                "path:a",
                "c.ts",
                10,
            ),
        ]);
        CorpusReport::build(
            configuration(),
            vec![
                entry("path:b", EntryStatus::Analyzed),
                entry("path:a", EntryStatus::Analyzed),
                entry("path:c", EntryStatus::Failed),
            ],
            vec![ReportEntryError {
                project: "path:c".into(),
                kind: EntryErrorKind::Timeout,
                message: "abandoned".into(),
            }],
            &groups,
            Tool::Eslint,
            Tool::Biome,
        )
    }

    #[test]
    fn arrays_are_sorted_and_the_summary_is_derived() {
        let report = report();
        assert_eq!(
            report
                .entries
                .iter()
                .map(|entry| entry.project.as_str())
                .collect::<Vec<_>>(),
            ["path:a", "path:b", "path:c"]
        );
        assert_eq!(report.summary.entries_total, 3);
        assert_eq!(report.summary.entries_analyzed, 2);
        assert_eq!(report.summary.entries_failed, 1);
        assert_eq!(report.summary.distinct_findings, 2);
        assert_eq!(report.summary.total_occurrences, 3);
        assert_eq!(report.findings[0].affected_projects, 2);
        assert_eq!(report.findings[0].projects, ["path:a", "path:b"]);
    }

    #[test]
    fn the_json_artifact_is_free_of_variable_and_absolute_values() {
        let rendered =
            serde_json::to_string_pretty(&report()).expect("the corpus report serializes");
        assert!(rendered.contains("\"schemaVersion\": 1"));
        for forbidden in ["durationMs", "timestamp", "elapsed", "/Users/", "C:\\"] {
            assert!(
                !rendered.contains(forbidden),
                "the artifact must not contain {forbidden}"
            );
        }
        let reparsed: serde_json::Value = serde_json::from_str(&rendered).expect("valid JSON");
        assert_eq!(reparsed["concordVersion"], env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn the_human_report_leads_with_the_summary_and_ends_with_errors() {
        let rendered = render(&report());
        assert!(rendered.starts_with("Concord corpus report"));
        assert!(
            rendered
                .contains("2 entries analyzed, 1 failed, 2 distinct findings, 3 total occurrences")
        );
        let findings = rendered.find("[1]").expect("first group");
        let errors = rendered.find("Entry errors").expect("errors section");
        assert!(findings < errors, "errors are rendered last");
        assert!(rendered.contains("timeout (1)"));
        assert!(rendered.contains("Affected projects   2 (2 occurrences)"));
    }

    #[test]
    fn an_empty_run_says_so() {
        let report = CorpusReport::build(
            configuration(),
            Vec::new(),
            Vec::new(),
            &[],
            Tool::Eslint,
            Tool::Biome,
        );
        assert!(render(&report).contains("No divergence was found."));
        assert!(!report.has_findings());
        assert!(!report.has_failures());
    }
}
