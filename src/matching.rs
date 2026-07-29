use std::collections::{BTreeMap, HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::config::AliasConfig;
use crate::model::Diagnostic;

#[derive(Debug, Clone)]
pub struct AliasTable {
    aliases: HashMap<String, String>,
}

impl AliasTable {
    pub fn new(configured: &[AliasConfig]) -> Self {
        let mut aliases = HashMap::new();
        for rule in [
            "no-debugger",
            "no-console",
            "no-undef",
            "eqeqeq",
            "no-unused-vars",
            "no-unreachable",
            "use-isnan",
            "constructor-super",
            "getter-return",
            "no-dupe-keys",
        ] {
            aliases.insert(rule.into(), rule.into());
        }
        for (alias, canonical) in [
            ("no-unused-variables", "no-unused-vars"),
            ("no-unused-variable", "no-unused-vars"),
            ("no-unreachable-code", "no-unreachable"),
            ("use-is-nan", "use-isnan"),
            ("use-constructor-super", "constructor-super"),
            ("use-getter-return", "getter-return"),
            ("no-duplicate-keys", "no-dupe-keys"),
            ("no-duplicate-object-keys", "no-dupe-keys"),
        ] {
            aliases.insert(alias.into(), canonical.into());
        }
        for group in configured {
            let values: Vec<String> = group.values().map(canonical_base).collect();
            if let Some(representative) = values.first() {
                let representative = aliases
                    .get(representative)
                    .cloned()
                    .unwrap_or_else(|| representative.clone());
                for value in values {
                    aliases.insert(value, representative.clone());
                }
            }
        }
        Self { aliases }
    }

    pub fn canonicalize(&self, rule: &str) -> String {
        let base = canonical_base(rule);
        self.aliases.get(&base).cloned().unwrap_or(base)
    }
}

impl Default for AliasTable {
    fn default() -> Self {
        Self::new(&[])
    }
}

pub fn canonical_base(rule: &str) -> String {
    let trimmed = rule.trim();
    let inner = trimmed
        .strip_suffix(')')
        .and_then(|without_end| without_end.rsplit_once('(').map(|(_, value)| value))
        .unwrap_or(trimmed);
    let leaf = inner.rsplit('/').next().unwrap_or(inner);
    to_kebab_case(leaf)
}

fn to_kebab_case(value: &str) -> String {
    let chars: Vec<char> = value.chars().collect();
    let mut result = String::new();
    for (index, character) in chars.iter().copied().enumerate() {
        if character == '_' || character == ' ' {
            if !result.ends_with('-') && !result.is_empty() {
                result.push('-');
            }
            continue;
        }
        if character == '-' {
            if !result.ends_with('-') && !result.is_empty() {
                result.push('-');
            }
            continue;
        }
        if character.is_ascii_uppercase() {
            let previous_is_lower = index > 0 && chars[index - 1].is_ascii_lowercase();
            let next_is_lower = chars.get(index + 1).is_some_and(char::is_ascii_lowercase);
            if !result.is_empty() && !result.ends_with('-') && (previous_is_lower || next_is_lower)
            {
                result.push('-');
            }
            result.push(character.to_ascii_lowercase());
        } else {
            result.push(character.to_ascii_lowercase());
        }
    }
    result.trim_matches('-').to_owned()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MatchKind {
    ExactMatch,
    ProbableMatch,
    SeverityChanged,
    RangeChanged,
    MessageChanged,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticMatch {
    pub kind: MatchKind,
    pub baseline: Diagnostic,
    pub candidate: Diagnostic,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MatchResult {
    pub matches: Vec<DiagnosticMatch>,
    pub baseline_only: Vec<Diagnostic>,
    pub candidate_only: Vec<Diagnostic>,
}

pub fn compare(mut baseline: Vec<Diagnostic>, mut candidate: Vec<Diagnostic>) -> MatchResult {
    sort_diagnostics(&mut baseline);
    sort_diagnostics(&mut candidate);
    let mut baseline_used = HashSet::new();
    let mut candidate_used = HashSet::new();
    let mut matches = Vec::new();

    let mut baseline_groups: BTreeMap<(String, String), Vec<usize>> = BTreeMap::new();
    let mut candidate_groups: BTreeMap<(String, String), Vec<usize>> = BTreeMap::new();
    for (index, diagnostic) in baseline.iter().enumerate() {
        if let Some(code) = &diagnostic.canonical_code {
            baseline_groups
                .entry((diagnostic.path.clone(), code.clone()))
                .or_default()
                .push(index);
        }
    }
    for (index, diagnostic) in candidate.iter().enumerate() {
        if let Some(code) = &diagnostic.canonical_code {
            candidate_groups
                .entry((diagnostic.path.clone(), code.clone()))
                .or_default()
                .push(index);
        }
    }

    for (key, baseline_indices) in baseline_groups {
        let Some(candidate_indices) = candidate_groups.get(&key) else {
            continue;
        };
        for baseline_index in baseline_indices {
            let best = candidate_indices
                .iter()
                .copied()
                .filter(|index| !candidate_used.contains(index))
                .min_by_key(|candidate_index| {
                    position_distance(&baseline[baseline_index], &candidate[*candidate_index])
                });
            if let Some(candidate_index) = best {
                baseline_used.insert(baseline_index);
                candidate_used.insert(candidate_index);
                matches.push(DiagnosticMatch {
                    kind: classify_correlated(
                        &baseline[baseline_index],
                        &candidate[candidate_index],
                    ),
                    baseline: baseline[baseline_index].clone(),
                    candidate: candidate[candidate_index].clone(),
                });
            }
        }
    }

    let mut candidates_by_path: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
    for (index, diagnostic) in candidate.iter().enumerate() {
        if !candidate_used.contains(&index) {
            candidates_by_path
                .entry(&diagnostic.path)
                .or_default()
                .push(index);
        }
    }
    for (baseline_index, baseline_diagnostic) in baseline.iter().enumerate() {
        if baseline_used.contains(&baseline_index) {
            continue;
        }
        let Some(indices) = candidates_by_path.get(baseline_diagnostic.path.as_str()) else {
            continue;
        };
        let probable = indices.iter().copied().find(|candidate_index| {
            if candidate_used.contains(candidate_index) {
                return false;
            }
            let candidate_diagnostic = &candidate[*candidate_index];
            if baseline_diagnostic.canonical_code.is_some()
                && baseline_diagnostic.canonical_code == candidate_diagnostic.canonical_code
            {
                return false;
            }
            positions_probably_match(baseline_diagnostic, candidate_diagnostic)
        });
        if let Some(candidate_index) = probable {
            baseline_used.insert(baseline_index);
            candidate_used.insert(candidate_index);
            matches.push(DiagnosticMatch {
                kind: MatchKind::ProbableMatch,
                baseline: baseline_diagnostic.clone(),
                candidate: candidate[candidate_index].clone(),
            });
        }
    }

    matches.sort_by(|left, right| match_sort_key(left).cmp(&match_sort_key(right)));
    let baseline_only = baseline
        .into_iter()
        .enumerate()
        .filter_map(|(index, diagnostic)| (!baseline_used.contains(&index)).then_some(diagnostic))
        .collect();
    let candidate_only = candidate
        .into_iter()
        .enumerate()
        .filter_map(|(index, diagnostic)| (!candidate_used.contains(&index)).then_some(diagnostic))
        .collect();
    MatchResult {
        matches,
        baseline_only,
        candidate_only,
    }
}

pub fn sort_diagnostics(diagnostics: &mut [Diagnostic]) {
    diagnostics.sort_by(|left, right| diagnostic_sort_key(left).cmp(&diagnostic_sort_key(right)));
}

fn diagnostic_sort_key(diagnostic: &Diagnostic) -> (&str, u32, u32, Option<&str>, &str, &str) {
    (
        &diagnostic.path,
        diagnostic
            .span
            .as_ref()
            .map_or(u32::MAX, |span| span.start_line),
        diagnostic
            .span
            .as_ref()
            .map_or(u32::MAX, |span| span.start_column),
        diagnostic.canonical_code.as_deref(),
        &diagnostic.message,
        &diagnostic.fingerprint,
    )
}

fn match_sort_key(value: &DiagnosticMatch) -> (&str, u32, u32, MatchKind, &str) {
    (
        &value.baseline.path,
        value
            .baseline
            .span
            .as_ref()
            .map_or(u32::MAX, |span| span.start_line),
        value
            .baseline
            .span
            .as_ref()
            .map_or(u32::MAX, |span| span.start_column),
        value.kind,
        &value.baseline.fingerprint,
    )
}

fn classify_correlated(baseline: &Diagnostic, candidate: &Diagnostic) -> MatchKind {
    if baseline.span != candidate.span {
        MatchKind::RangeChanged
    } else if baseline.severity != candidate.severity {
        MatchKind::SeverityChanged
    } else if baseline.message != candidate.message {
        MatchKind::MessageChanged
    } else {
        MatchKind::ExactMatch
    }
}

fn position_distance(left: &Diagnostic, right: &Diagnostic) -> (u32, u32) {
    match (&left.span, &right.span) {
        (Some(left), Some(right)) => (
            left.start_line.abs_diff(right.start_line),
            left.start_column.abs_diff(right.start_column),
        ),
        (None, None) => (0, 0),
        _ => (u32::MAX, u32::MAX),
    }
}

fn positions_probably_match(left: &Diagnostic, right: &Diagnostic) -> bool {
    match (&left.span, &right.span) {
        (Some(left), Some(right)) => left.overlaps_or_same_line(right),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use crate::model::{Diagnostic, DiagnosticData, Severity, Span, Tool};

    use super::{AliasTable, MatchKind, canonical_base, compare};

    fn diagnostic(tool: Tool, code: Option<&str>, line: u32) -> Diagnostic {
        let aliases = AliasTable::default();
        Diagnostic::new(
            tool,
            "src/app.ts",
            DiagnosticData {
                code: code.map(str::to_owned),
                canonical_code: code.map(|value| aliases.canonicalize(value)),
                severity: Severity::Error,
                message: "message".into(),
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

    #[test]
    fn canonicalizes_namespaces_and_case() {
        assert_eq!(canonical_base("lint/suspicious/noDebugger"), "no-debugger");
        assert_eq!(canonical_base("eslint/no_debugger"), "no-debugger");
        assert_eq!(canonical_base("eslint(noDebugger)"), "no-debugger");
    }

    #[test]
    fn applies_builtin_aliases() {
        let aliases = AliasTable::default();
        assert_eq!(
            aliases.canonicalize("lint/correctness/noUnusedVariables"),
            "no-unused-vars"
        );
    }

    #[test]
    fn applies_configured_aliases() {
        let aliases = AliasTable::new(&[crate::config::AliasConfig {
            eslint: Some("custom-eslint-rule".into()),
            biome: Some("lint/custom/customBiomeRule".into()),
            oxlint: None,
        }]);
        assert_eq!(
            aliases.canonicalize("lint/custom/customBiomeRule"),
            "custom-eslint-rule"
        );
    }

    #[test]
    fn exact_match_is_found() {
        let result = compare(
            vec![diagnostic(Tool::Eslint, Some("no-debugger"), 2)],
            vec![diagnostic(
                Tool::Biome,
                Some("lint/suspicious/noDebugger"),
                2,
            )],
        );
        assert_eq!(result.matches[0].kind, MatchKind::ExactMatch);
    }

    #[test]
    fn probable_match_requires_position_but_not_code() {
        let result = compare(
            vec![diagnostic(Tool::Eslint, Some("no-console"), 3)],
            vec![diagnostic(Tool::Biome, Some("someOtherRule"), 3)],
        );
        assert_eq!(result.matches[0].kind, MatchKind::ProbableMatch);
    }

    #[test]
    fn order_is_deterministic() {
        let first = compare(
            vec![
                diagnostic(Tool::Eslint, Some("no-console"), 5),
                diagnostic(Tool::Eslint, Some("no-debugger"), 2),
            ],
            Vec::new(),
        );
        assert_eq!(
            first.baseline_only[0].span.as_ref().map(|s| s.start_line),
            Some(2)
        );
    }
}
