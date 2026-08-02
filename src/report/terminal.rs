use std::fmt::Write;

use crate::capabilities::ComparisonPlan;
use crate::matching::MatchKind;
use crate::model::Diagnostic;
use crate::report::{
    ComparisonProfile, FormatComparisonStatus, FormatReport, LintReport, PlanReport,
};

pub fn plan(report: &PlanReport) -> String {
    let mut output = String::new();
    let _ = writeln!(output, "Concord comparison plan\n");
    let _ = writeln!(output, "Mode        {}", report.mode);
    let _ = writeln!(output, "Baseline    {}", report.baseline_tool);
    let _ = writeln!(output, "Candidate   {}\n", report.candidate_tool);
    render_file_counts(&mut output, &report.plan);
    if report.mode == "lint" {
        let _ = writeln!(output, "\nRule mappings");
        let _ = writeln!(
            output,
            "  Exact mappings         {}",
            report.configured_rule_mappings.exact
        );
        let _ = writeln!(
            output,
            "  Approximate mappings   {}",
            report.configured_rule_mappings.approximate
        );
        for mapping in &report.rule_mappings {
            let _ = writeln!(
                output,
                "\n  {}:{} <-> {}:{} ({})",
                mapping.baseline_tool.config_key(),
                mapping.baseline,
                mapping.candidate_tool.config_key(),
                mapping.candidate,
                match mapping.confidence {
                    crate::matching::MappingConfidence::Exact => "exact",
                    crate::matching::MappingConfidence::Approximate => "approximate",
                }
            );
            if let Some(notes) = &mapping.notes {
                let _ = writeln!(output, "    {notes}");
            }
        }
    }
    for tool in report.plan.tools.iter().filter(|tool| !tool.available) {
        let _ = writeln!(output, "\nMissing tool\n  {}", tool.tool);
        if let Some(reason) = &tool.reason {
            let _ = writeln!(output, "    {}", first_line(reason));
        }
    }
    render_dispositions(&mut output, "Unsupported", &report.plan.unsupported);
    render_dispositions(
        &mut output,
        "Skipped by configuration",
        &report.plan.skipped,
    );
    output
}

pub fn lint(report: &LintReport) -> String {
    let mut output = String::new();
    let baseline_version = first_line(&report.baseline.version);
    let candidate_version = first_line(&report.candidate.version);
    let _ = writeln!(output, "Concord lint comparison\n");
    let profile_line = match report.profile {
        ComparisonProfile::Raw => "raw",
        ComparisonProfile::Comparable => "comparable",
    };
    let _ = writeln!(output, "Profile      {profile_line}");
    let _ = writeln!(
        output,
        "Baseline     {} {}",
        report.baseline.tool, baseline_version
    );
    let _ = writeln!(
        output,
        "Candidate    {} {}\n",
        report.candidate.tool, candidate_version
    );
    render_file_counts(&mut output, &report.plan);
    let mapping = &report.mapping_summary;
    let _ = writeln!(output, "\nObserved rules");
    let _ = writeln!(
        output,
        "  Baseline             {}",
        mapping.observed_baseline_rule_ids.len()
    );
    let _ = writeln!(
        output,
        "  Candidate            {}",
        mapping.observed_candidate_rule_ids.len()
    );
    let _ = writeln!(
        output,
        "  Exact mapped         {}",
        mapping.exact_mapped_rule_ids.len()
    );
    let _ = writeln!(
        output,
        "  Approximate mapped   {}",
        mapping.approximate_mapped_rule_ids.len()
    );
    let _ = writeln!(
        output,
        "  Unmapped baseline    {}",
        mapping.unmapped_baseline_rule_ids.len()
    );
    let _ = writeln!(
        output,
        "  Unmapped candidate   {}",
        mapping.unmapped_candidate_rule_ids.len()
    );
    let summary = &report.comparable_summary;
    let _ = writeln!(output, "\nComparable diagnostics");
    let _ = writeln!(output, "  Exact matches         {}", summary.exact_matches);
    let _ = writeln!(
        output,
        "  Approximate matches   {}",
        summary.approximate_matches
    );
    let _ = writeln!(output, "  Baseline only         {}", summary.baseline_only);
    let _ = writeln!(output, "  Candidate only        {}", summary.candidate_only);
    if report.profile == ComparisonProfile::Raw {
        let raw = &report.raw_summary;
        let _ = writeln!(output, "\nRaw diagnostics");
        let _ = writeln!(
            output,
            "  Baseline              {}",
            raw.baseline_diagnostics
        );
        let _ = writeln!(
            output,
            "  Candidate             {}",
            raw.candidate_diagnostics
        );
        let _ = writeln!(output, "  Unmapped baseline     {}", raw.unmapped_baseline);
        let _ = writeln!(output, "  Unmapped candidate    {}", raw.unmapped_candidate);
    }
    let _ = writeln!(
        output,
        "\nObserved mapping coverage: {:.1}%",
        mapping.observed_rule_mapping_coverage
    );
    let _ = writeln!(
        output,
        "Comparable file coverage: {:.1}%",
        summary.comparable_file_coverage
    );
    let _ = writeln!(output, "Exact agreement: {:.1}%", summary.exact_agreement);
    let _ = writeln!(
        output,
        "Comparable agreement: {:.1}%",
        summary.comparable_agreement
    );

    let changed: Vec<_> = report
        .matches
        .iter()
        .filter(|item| !matches!(item.kind, MatchKind::ExactMatch))
        .take(20)
        .collect();
    render_diagnostics(&mut output, "BASELINE ONLY", &report.baseline_only);
    render_diagnostics(&mut output, "CANDIDATE ONLY", &report.candidate_only);
    render_diagnostics(&mut output, "UNMAPPED BASELINE", &report.unmapped_baseline);
    render_diagnostics(
        &mut output,
        "UNMAPPED CANDIDATE",
        &report.unmapped_candidate,
    );
    for item in changed {
        let _ = writeln!(output, "\n{}", kind_label(item.kind));
        render_diagnostic(&mut output, &item.baseline);
        let _ = writeln!(
            output,
            "  candidate: {}",
            item.candidate.message.replace('\n', " ")
        );
    }
    render_dispositions(&mut output, "Unsupported", &report.unsupported_files);
    render_dispositions(
        &mut output,
        "Skipped by configuration",
        &report.skipped_files,
    );
    output
}

pub fn format(report: &FormatReport) -> String {
    let mut output = String::new();
    let _ = writeln!(output, "Concord format comparison\n");
    let _ = writeln!(output, "Baseline   {}", report.baseline_tool);
    let _ = writeln!(output, "Candidate  {}\n", report.candidate_tool);
    let _ = writeln!(output, "Files");
    let _ = writeln!(output, "  Discovered      {}", report.summary.discovered);
    let _ = writeln!(output, "  Compared        {}", report.summary.compared);
    let _ = writeln!(output, "  Identical       {}", report.summary.identical);
    let _ = writeln!(output, "  Different       {}", report.summary.different);
    let _ = writeln!(output, "  Unsupported     {}", report.summary.unsupported);
    let _ = writeln!(output, "  Skipped         {}", report.summary.skipped);
    let _ = writeln!(output, "  Failed          {}", report.summary.failed);
    for file in report
        .files
        .iter()
        .filter(|file| file.status != FormatComparisonStatus::Identical)
        .take(20)
    {
        let _ = writeln!(output, "\n{}  {:?}", file.path, file.status);
        if let Some(diff) = &file.diff {
            let _ = writeln!(output, "{diff}");
        }
        for (side, outcome) in [("baseline", &file.baseline), ("candidate", &file.candidate)] {
            if let Some(reason) = &outcome.reason {
                let _ = writeln!(output, "  {side}: {reason}");
            }
            if let Some(error) = &outcome.error {
                let _ = writeln!(output, "  {side} error: {error}");
            }
            if let Some(stderr) = &outcome.stderr {
                let _ = writeln!(output, "  {side} stderr: {stderr}");
            }
        }
    }
    output
}

fn render_file_counts(output: &mut String, plan: &ComparisonPlan) {
    let _ = writeln!(output, "Files");
    let _ = writeln!(output, "  Discovered       {}", plan.discovered_count());
    let _ = writeln!(output, "  Comparable       {}", plan.comparable_count());
    let _ = writeln!(output, "  Unsupported      {}", plan.unsupported_count());
    let _ = writeln!(output, "  Skipped          {}", plan.skipped_count());
}

fn render_dispositions(
    output: &mut String,
    label: &str,
    dispositions: &[crate::capabilities::FileDisposition],
) {
    if dispositions.is_empty() {
        return;
    }
    let _ = writeln!(output, "\n{label}");
    for item in dispositions.iter().take(20) {
        let _ = writeln!(output, "\n  {}", item.path);
        let _ = writeln!(output, "    {:?}: {}", item.side, item.tool);
        let _ = writeln!(output, "    Reason: {}", item.reason);
    }
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
        diagnostic.code.as_deref().unwrap_or("<no rule>")
    );
    let _ = writeln!(output, "  {}", diagnostic.message.replace('\n', " "));
}

fn kind_label(kind: MatchKind) -> &'static str {
    match kind {
        MatchKind::ExactMatch => "EXACT MATCH",
        MatchKind::ApproximateRuleMatch => "APPROXIMATE RULE MATCH",
        MatchKind::ProbableMatch => "PROBABLE MATCH",
        MatchKind::SeverityChanged => "SEVERITY CHANGED",
        MatchKind::RangeChanged => "RANGE CHANGED",
        MatchKind::MessageChanged => "MESSAGE CHANGED",
    }
}

fn first_line(value: &str) -> &str {
    value.lines().next().unwrap_or("unknown")
}
