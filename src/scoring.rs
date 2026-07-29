use serde::{Deserialize, Serialize};

use crate::matching::{MatchKind, MatchResult};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LintSummary {
    pub baseline_diagnostics: usize,
    pub candidate_diagnostics: usize,
    pub exact_matches: usize,
    pub probable_matches: usize,
    pub baseline_only: usize,
    pub candidate_only: usize,
    pub severity_changes: usize,
    pub range_changes: usize,
    pub message_changes: usize,
    pub tool_failures: usize,
    pub baseline_coverage: f64,
    pub candidate_precision: f64,
    pub exact_agreement: f64,
    pub probable_agreement: f64,
}

pub fn calculate(result: &MatchResult) -> LintSummary {
    let count = |kind| {
        result
            .matches
            .iter()
            .filter(|item| item.kind == kind)
            .count()
    };
    let exact = count(MatchKind::ExactMatch);
    let probable = count(MatchKind::ProbableMatch);
    let severity_changes = count(MatchKind::SeverityChanged);
    let range_changes = count(MatchKind::RangeChanged);
    let message_changes = count(MatchKind::MessageChanged);
    let baseline_total = result.baseline_only.len() + result.matches.len();
    let candidate_total = result.candidate_only.len() + result.matches.len();
    LintSummary {
        baseline_diagnostics: baseline_total,
        candidate_diagnostics: candidate_total,
        exact_matches: exact,
        probable_matches: probable,
        baseline_only: result.baseline_only.len(),
        candidate_only: result.candidate_only.len(),
        severity_changes,
        range_changes,
        message_changes,
        tool_failures: 0,
        baseline_coverage: ratio(exact, baseline_total),
        candidate_precision: ratio(exact, candidate_total),
        exact_agreement: agreement(exact, baseline_total, candidate_total),
        probable_agreement: agreement(exact + probable, baseline_total, candidate_total),
    }
}

fn ratio(numerator: usize, denominator: usize) -> f64 {
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
    use crate::matching::MatchResult;

    #[test]
    fn empty_sets_have_complete_vacuous_agreement() {
        let summary = calculate(&MatchResult {
            matches: Vec::new(),
            baseline_only: Vec::new(),
            candidate_only: Vec::new(),
        });
        assert_eq!(summary.baseline_coverage, 100.0);
        assert_eq!(summary.candidate_precision, 100.0);
        assert_eq!(summary.exact_agreement, 100.0);
    }
}
