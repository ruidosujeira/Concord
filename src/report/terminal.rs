use std::fmt::Write;

use crate::matching::MatchKind;
use crate::model::Diagnostic;
use crate::report::{FormatReport, FormatStatus, LintReport};

pub fn lint(report: &LintReport) -> String {
    let mut output = String::new();
    let baseline_version = first_line(&report.baseline.version);
    let candidate_version = first_line(&report.candidate.version);
    let _ = writeln!(output, "Concord lint comparison\n");
    let _ = writeln!(
        output,
        "Baseline   {} {}",
        report.baseline.tool, baseline_version
    );
    let _ = writeln!(
        output,
        "Candidate  {} {}",
        report.candidate.tool, candidate_version
    );
    let _ = writeln!(output, "Files      {}\n", report.files);
    let summary = &report.summary;
    let _ = writeln!(output, "Diagnostics");
    let _ = writeln!(output, "  Baseline:       {}", summary.baseline_diagnostics);
    let _ = writeln!(
        output,
        "  Candidate:      {}",
        summary.candidate_diagnostics
    );
    let _ = writeln!(output, "  Exact matches:  {}", summary.exact_matches);
    let _ = writeln!(output, "  Probable:       {}", summary.probable_matches);
    let _ = writeln!(output, "  Baseline only:  {}", summary.baseline_only);
    let _ = writeln!(output, "  Candidate only: {}", summary.candidate_only);
    let _ = writeln!(output, "  Severity:       {}", summary.severity_changes);
    let _ = writeln!(output, "  Range:          {}", summary.range_changes);
    let _ = writeln!(output, "  Message:        {}\n", summary.message_changes);
    let _ = writeln!(
        output,
        "Exact agreement:      {:.1}%",
        summary.exact_agreement
    );
    let _ = writeln!(
        output,
        "Baseline coverage:    {:.1}%",
        summary.baseline_coverage
    );
    let _ = writeln!(
        output,
        "Candidate precision:  {:.1}%",
        summary.candidate_precision
    );
    let _ = writeln!(
        output,
        "Probable agreement:   {:.1}%",
        summary.probable_agreement
    );

    let changed: Vec<_> = report
        .matches
        .iter()
        .filter(|item| item.kind != MatchKind::ExactMatch)
        .take(20)
        .collect();
    if !report.baseline_only.is_empty() || !report.candidate_only.is_empty() || !changed.is_empty()
    {
        let _ = writeln!(output, "\nImportant differences");
    }
    render_diagnostics(&mut output, "BASELINE ONLY", &report.baseline_only);
    render_diagnostics(&mut output, "CANDIDATE ONLY", &report.candidate_only);
    for item in changed {
        let _ = writeln!(output, "\n{}", kind_label(item.kind));
        render_diagnostic(&mut output, &item.baseline);
        let _ = writeln!(
            output,
            "  candidate: {}",
            item.candidate.message.replace('\n', " ")
        );
    }
    output
}

pub fn format(report: &FormatReport) -> String {
    let mut output = String::new();
    let _ = writeln!(output, "Concord format comparison\n");
    let _ = writeln!(output, "Baseline   {}", report.baseline_tool);
    let _ = writeln!(output, "Candidate  {}", report.candidate_tool);
    let _ = writeln!(output, "Files      {}\n", report.summary.files);
    let _ = writeln!(output, "Results");
    let _ = writeln!(
        output,
        "  Identical:                {}",
        report.summary.identical
    );
    let _ = writeln!(
        output,
        "  Different:                {}",
        report.summary.different
    );
    let _ = writeln!(
        output,
        "  Baseline non-idempotent:  {}",
        report.summary.baseline_non_idempotent
    );
    let _ = writeln!(
        output,
        "  Candidate non-idempotent: {}",
        report.summary.candidate_non_idempotent
    );
    let _ = writeln!(
        output,
        "  Tool failures:            {}",
        report.summary.tool_failures
    );
    for file in report
        .files
        .iter()
        .filter(|file| file.status != FormatStatus::Identical)
        .take(20)
    {
        let _ = writeln!(output, "\n{}  {:?}", file.path, file.status);
        if let Some(diff) = &file.diff {
            let _ = writeln!(output, "{diff}");
        }
        if let Some(error) = &file.baseline.error {
            let _ = writeln!(output, "  baseline error: {error}");
        }
        if let Some(error) = &file.candidate.error {
            let _ = writeln!(output, "  candidate error: {error}");
        }
    }
    output
}

fn render_diagnostics(output: &mut String, label: &str, diagnostics: &[Diagnostic]) {
    if diagnostics.is_empty() {
        return;
    }
    let _ = writeln!(output, "\n{label}");
    for diagnostic in diagnostics.iter().take(20) {
        render_diagnostic(output, diagnostic);
    }
}

fn render_diagnostic(output: &mut String, diagnostic: &Diagnostic) {
    let location = diagnostic.span.as_ref().map_or_else(
        || diagnostic.path.clone(),
        |span| {
            format!(
                "{}:{}:{}",
                diagnostic.path, span.start_line, span.start_column
            )
        },
    );
    let _ = writeln!(output, "  {location}");
    let _ = writeln!(
        output,
        "  {}",
        diagnostic.canonical_code.as_deref().unwrap_or("<no rule>")
    );
    let _ = writeln!(output, "  {}", diagnostic.message.replace('\n', " "));
}

fn kind_label(kind: MatchKind) -> &'static str {
    match kind {
        MatchKind::ExactMatch => "EXACT MATCH",
        MatchKind::ProbableMatch => "PROBABLE MATCH",
        MatchKind::SeverityChanged => "SEVERITY CHANGED",
        MatchKind::RangeChanged => "RANGE CHANGED",
        MatchKind::MessageChanged => "MESSAGE CHANGED",
    }
}

fn first_line(value: &str) -> &str {
    value.lines().next().unwrap_or("unknown")
}
