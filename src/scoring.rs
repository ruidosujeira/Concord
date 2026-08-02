use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::matching::{
    MappingConfidence, MatchKind, MatchResult, RuleMappingTable, observed_rule_ids,
};
use crate::model::{Diagnostic, Tool};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RawLintSummary {
    pub baseline_diagnostics: usize,
    pub candidate_diagnostics: usize,
    pub exact_matches: usize,
    pub approximate_matches: usize,
    pub probable_matches: usize,
    pub baseline_only: usize,
    pub candidate_only: usize,
    pub unmapped_baseline: usize,
    pub unmapped_candidate: usize,
    pub severity_changes: usize,
    pub range_changes: usize,
    pub message_changes: usize,
    pub tool_failures: usize,
    pub exact_agreement: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ComparableLintSummary {
    pub baseline_diagnostics: usize,
    pub candidate_diagnostics: usize,
    pub exact_matches: usize,
    pub approximate_matches: usize,
    pub probable_matches: usize,
    pub baseline_only: usize,
    pub candidate_only: usize,
    pub severity_changes: usize,
    pub range_changes: usize,
    pub message_changes: usize,
    pub exact_agreement: f64,
    pub comparable_agreement: f64,
    pub comparable_file_coverage: f64,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ObservedRuleId {
    pub tool: Tool,
    pub code: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MappingSummary {
    pub observed_baseline_rule_ids: Vec<String>,
    pub observed_candidate_rule_ids: Vec<String>,
    pub exact_mapped_rule_ids: Vec<ObservedRuleId>,
    pub approximate_mapped_rule_ids: Vec<ObservedRuleId>,
    pub unmapped_baseline_rule_ids: Vec<String>,
    pub unmapped_candidate_rule_ids: Vec<String>,
    pub observed_rule_mapping_coverage: f64,
}

#[allow(clippy::too_many_arguments)]
pub fn calculate(
    result: &MatchResult,
    baseline_diagnostics: &[Diagnostic],
    candidate_diagnostics: &[Diagnostic],
    mappings: &RuleMappingTable,
    baseline_tool: Tool,
    candidate_tool: Tool,
    discovered_files: usize,
    comparable_files: usize,
) -> (RawLintSummary, ComparableLintSummary, MappingSummary) {
    let count = |kind| {
        result
            .matches
            .iter()
            .filter(|item| item.kind == kind)
            .count()
    };
    let exact = count(MatchKind::ExactMatch);
    let approximate = count(MatchKind::ApproximateRuleMatch);
    let probable = count(MatchKind::ProbableMatch);
    let severity_changes = count(MatchKind::SeverityChanged);
    let range_changes = count(MatchKind::RangeChanged);
    let message_changes = count(MatchKind::MessageChanged);
    let raw = RawLintSummary {
        baseline_diagnostics: baseline_diagnostics.len(),
        candidate_diagnostics: candidate_diagnostics.len(),
        exact_matches: exact,
        approximate_matches: approximate,
        probable_matches: probable,
        baseline_only: result.baseline_only.len(),
        candidate_only: result.candidate_only.len(),
        unmapped_baseline: result.unmapped_baseline.len(),
        unmapped_candidate: result.unmapped_candidate.len(),
        severity_changes,
        range_changes,
        message_changes,
        tool_failures: 0,
        exact_agreement: agreement(
            exact,
            baseline_diagnostics.len(),
            candidate_diagnostics.len(),
        ),
    };
    let baseline_comparable = result.matches.len() + result.baseline_only.len();
    let candidate_comparable = result.matches.len() + result.candidate_only.len();
    let comparable = ComparableLintSummary {
        baseline_diagnostics: baseline_comparable,
        candidate_diagnostics: candidate_comparable,
        exact_matches: exact,
        approximate_matches: approximate,
        probable_matches: probable,
        baseline_only: result.baseline_only.len(),
        candidate_only: result.candidate_only.len(),
        severity_changes,
        range_changes,
        message_changes,
        exact_agreement: agreement(exact, baseline_comparable, candidate_comparable),
        comparable_agreement: agreement(
            exact + approximate + probable,
            baseline_comparable,
            candidate_comparable,
        ),
        comparable_file_coverage: ratio(comparable_files, discovered_files),
    };
    let mapping = mapping_summary(
        result,
        baseline_diagnostics,
        candidate_diagnostics,
        mappings,
        baseline_tool,
        candidate_tool,
    );
    (raw, comparable, mapping)
}

fn mapping_summary(
    result: &MatchResult,
    baseline: &[Diagnostic],
    candidate: &[Diagnostic],
    mappings: &RuleMappingTable,
    baseline_tool: Tool,
    candidate_tool: Tool,
) -> MappingSummary {
    let observed_baseline_rule_ids = observed_rule_ids(baseline);
    let observed_candidate_rule_ids = observed_rule_ids(candidate);
    let mut exact = BTreeSet::new();
    let mut approximate = BTreeSet::new();
    let mut unmapped_baseline_rule_ids = Vec::new();
    let mut unmapped_candidate_rule_ids = Vec::new();
    for item in &result.matches {
        let destination = match item.mapping_confidence {
            Some(MappingConfidence::Approximate) => &mut approximate,
            Some(MappingConfidence::Exact) => &mut exact,
            None => continue,
        };
        for diagnostic in [&item.baseline, &item.candidate] {
            if let Some(code) = &diagnostic.code {
                destination.insert(ObservedRuleId {
                    tool: diagnostic.tool,
                    code: code.clone(),
                });
            }
        }
    }
    classify_observed_rules(
        baseline_tool,
        candidate_tool,
        &observed_baseline_rule_ids,
        mappings,
        &mut exact,
        &mut approximate,
        &mut unmapped_baseline_rule_ids,
    );
    classify_observed_rules(
        candidate_tool,
        baseline_tool,
        &observed_candidate_rule_ids,
        mappings,
        &mut exact,
        &mut approximate,
        &mut unmapped_candidate_rule_ids,
    );
    let observed_total = observed_baseline_rule_ids.len() + observed_candidate_rule_ids.len();
    let mapped_total = exact.len() + approximate.len();
    MappingSummary {
        observed_baseline_rule_ids,
        observed_candidate_rule_ids,
        exact_mapped_rule_ids: exact.into_iter().collect(),
        approximate_mapped_rule_ids: approximate.into_iter().collect(),
        unmapped_baseline_rule_ids,
        unmapped_candidate_rule_ids,
        observed_rule_mapping_coverage: ratio(mapped_total, observed_total),
    }
}

fn classify_observed_rules(
    tool: Tool,
    other_tool: Tool,
    observed: &[String],
    mappings: &RuleMappingTable,
    exact: &mut BTreeSet<ObservedRuleId>,
    approximate: &mut BTreeSet<ObservedRuleId>,
    unmapped: &mut Vec<String>,
) {
    for code in observed {
        let rule = ObservedRuleId {
            tool,
            code: code.clone(),
        };
        if exact.contains(&rule) || approximate.contains(&rule) {
            continue;
        }
        match mappings.confidence_for_rule(tool, code, other_tool) {
            Some(MappingConfidence::Exact) => {
                exact.insert(rule);
            }
            Some(MappingConfidence::Approximate) => {
                approximate.insert(rule);
            }
            None => unmapped.push(code.clone()),
        }
    }
}

pub fn ratio(numerator: usize, denominator: usize) -> f64 {
    if denominator == 0 {
        100.0
    } else {
        numerator as f64 * 100.0 / denominator as f64
    }
}

fn agreement(matches: usize, baseline: usize, candidate: usize) -> f64 {
    let denominator = baseline + candidate;
    if denominator == 0 {
        100.0
    } else {
        2.0 * matches as f64 * 100.0 / denominator as f64
    }
}

#[cfg(test)]
mod tests {
    use super::calculate;
    use crate::matching::{MatchResult, RuleMappingTable};
    use crate::model::Tool;

    #[test]
    fn empty_sets_have_explicit_vacuous_coverage() {
        let result = MatchResult {
            matches: Vec::new(),
            baseline_only: Vec::new(),
            candidate_only: Vec::new(),
            unmapped_baseline: Vec::new(),
            unmapped_candidate: Vec::new(),
        };
        let (raw, comparable, mapping) = calculate(
            &result,
            &[],
            &[],
            &RuleMappingTable::default(),
            Tool::Eslint,
            Tool::Biome,
            0,
            0,
        );
        assert_eq!(raw.exact_agreement, 100.0);
        assert_eq!(comparable.exact_agreement, 100.0);
        assert_eq!(comparable.comparable_file_coverage, 100.0);
        assert_eq!(mapping.observed_rule_mapping_coverage, 100.0);
    }
}
