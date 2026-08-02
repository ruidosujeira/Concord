use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::config::{AliasConfig, RuleMappingConfig};
use crate::model::{Diagnostic, Tool};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MappingConfidence {
    Exact,
    Approximate,
}

#[derive(Debug, Clone)]
pub struct AliasTable {
    aliases: HashMap<String, String>,
}

impl AliasTable {
    pub fn new(configured: &[AliasConfig]) -> Self {
        let mut aliases = built_in_aliases();
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

fn built_in_aliases() -> HashMap<String, String> {
    let mut aliases = HashMap::new();
    for rule in built_in_rule_catalog() {
        aliases.insert((*rule).into(), (*rule).into());
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
    aliases
}

fn built_in_rule_catalog() -> &'static [&'static str] {
    &[
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
    ]
}

#[derive(Debug, Clone)]
struct RuleMapping {
    left_tool: Tool,
    left_code: String,
    right_tool: Tool,
    right_code: String,
    confidence: MappingConfidence,
    configured: bool,
}

#[derive(Debug, Clone)]
pub struct RuleMappingTable {
    mappings: Vec<RuleMapping>,
    aliases: AliasTable,
}

impl RuleMappingTable {
    pub fn new(rules: &[RuleMappingConfig], aliases: &[AliasConfig]) -> Self {
        let mut mappings = Vec::new();
        for rule in rules {
            mappings.push(ordered_mapping(
                rule.baseline_tool,
                &rule.baseline,
                rule.candidate_tool,
                &rule.candidate,
                rule.confidence,
                true,
            ));
        }
        for alias in aliases {
            let values: Vec<_> = alias.tool_values().collect();
            for left in 0..values.len() {
                for right in (left + 1)..values.len() {
                    mappings.push(ordered_mapping(
                        values[left].0,
                        values[left].1,
                        values[right].0,
                        values[right].1,
                        MappingConfidence::Exact,
                        true,
                    ));
                }
            }
        }
        mappings.sort_by(|left, right| mapping_key(left).cmp(&mapping_key(right)));
        mappings.dedup_by(|left, right| mapping_key(left) == mapping_key(right));
        Self {
            mappings,
            aliases: AliasTable::new(aliases),
        }
    }

    pub fn confidence_between(
        &self,
        baseline: &Diagnostic,
        candidate: &Diagnostic,
    ) -> Option<MappingConfidence> {
        let baseline_code = baseline.code.as_deref()?;
        let candidate_code = candidate.code.as_deref()?;
        if let Some(confidence) =
            self.explicit_confidence(baseline.tool, baseline_code, candidate.tool, candidate_code)
        {
            return Some(confidence);
        }
        let baseline_canonical = self.aliases.canonicalize(baseline_code);
        let candidate_canonical = self.aliases.canonicalize(candidate_code);
        (baseline_canonical == candidate_canonical).then_some(MappingConfidence::Exact)
    }

    pub fn confidence_for_rule(
        &self,
        tool: Tool,
        code: &str,
        other_tool: Tool,
    ) -> Option<MappingConfidence> {
        let configured = self
            .mappings
            .iter()
            .filter_map(|mapping| endpoint_confidence(mapping, tool, code, other_tool))
            .min();
        if configured.is_some() {
            return configured;
        }
        let canonical = self.aliases.canonicalize(code);
        built_in_rule_catalog()
            .contains(&canonical.as_str())
            .then_some(MappingConfidence::Exact)
    }

    pub fn configured_counts(&self, left: Tool, right: Tool) -> (usize, usize) {
        let relevant = self.mappings.iter().filter(|mapping| {
            mapping.configured
                && ((mapping.left_tool == left && mapping.right_tool == right)
                    || (mapping.left_tool == right && mapping.right_tool == left))
        });
        let mut exact = 0;
        let mut approximate = 0;
        for mapping in relevant {
            match mapping.confidence {
                MappingConfidence::Exact => exact += 1,
                MappingConfidence::Approximate => approximate += 1,
            }
        }
        (exact, approximate)
    }

    fn explicit_confidence(
        &self,
        left_tool: Tool,
        left_code: &str,
        right_tool: Tool,
        right_code: &str,
    ) -> Option<MappingConfidence> {
        self.mappings.iter().find_map(|mapping| {
            let direct = mapping.left_tool == left_tool
                && mapping.right_tool == right_tool
                && rule_id_eq(&mapping.left_code, left_code)
                && rule_id_eq(&mapping.right_code, right_code);
            let reverse = mapping.left_tool == right_tool
                && mapping.right_tool == left_tool
                && rule_id_eq(&mapping.left_code, right_code)
                && rule_id_eq(&mapping.right_code, left_code);
            (direct || reverse).then_some(mapping.confidence)
        })
    }
}

impl Default for RuleMappingTable {
    fn default() -> Self {
        Self::new(&[], &[])
    }
}

fn endpoint_confidence(
    mapping: &RuleMapping,
    tool: Tool,
    code: &str,
    other_tool: Tool,
) -> Option<MappingConfidence> {
    let direct = mapping.left_tool == tool
        && mapping.right_tool == other_tool
        && rule_id_eq(&mapping.left_code, code);
    let reverse = mapping.right_tool == tool
        && mapping.left_tool == other_tool
        && rule_id_eq(&mapping.right_code, code);
    (direct || reverse).then_some(mapping.confidence)
}

fn ordered_mapping(
    left_tool: Tool,
    left_code: &str,
    right_tool: Tool,
    right_code: &str,
    confidence: MappingConfidence,
    configured: bool,
) -> RuleMapping {
    let left = (left_tool, left_code.trim().to_owned());
    let right = (right_tool, right_code.trim().to_owned());
    let (left, right) = if left <= right {
        (left, right)
    } else {
        (right, left)
    };
    RuleMapping {
        left_tool: left.0,
        left_code: left.1,
        right_tool: right.0,
        right_code: right.1,
        confidence,
        configured,
    }
}

fn mapping_key(mapping: &RuleMapping) -> (Tool, &str, Tool, &str, MappingConfidence, bool) {
    (
        mapping.left_tool,
        &mapping.left_code,
        mapping.right_tool,
        &mapping.right_code,
        mapping.confidence,
        mapping.configured,
    )
}

fn rule_id_eq(left: &str, right: &str) -> bool {
    left.trim().eq_ignore_ascii_case(right.trim())
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
        if character == '_' || character == ' ' || character == '-' {
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
    ApproximateRuleMatch,
    ProbableMatch,
    SeverityChanged,
    RangeChanged,
    MessageChanged,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticMatch {
    pub kind: MatchKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mapping_confidence: Option<MappingConfidence>,
    pub baseline: Diagnostic,
    pub candidate: Diagnostic,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MatchResult {
    pub matches: Vec<DiagnosticMatch>,
    pub baseline_only: Vec<Diagnostic>,
    pub candidate_only: Vec<Diagnostic>,
    pub unmapped_baseline: Vec<Diagnostic>,
    pub unmapped_candidate: Vec<Diagnostic>,
}

pub fn compare(baseline: Vec<Diagnostic>, candidate: Vec<Diagnostic>) -> MatchResult {
    compare_with_mappings(
        baseline,
        candidate,
        &RuleMappingTable::default(),
        Tool::Eslint,
        Tool::Biome,
    )
}

pub fn compare_with_mappings(
    mut baseline: Vec<Diagnostic>,
    mut candidate: Vec<Diagnostic>,
    mappings: &RuleMappingTable,
    baseline_tool: Tool,
    candidate_tool: Tool,
) -> MatchResult {
    sort_diagnostics(&mut baseline);
    sort_diagnostics(&mut candidate);
    let mut baseline_used = HashSet::new();
    let mut candidate_used = HashSet::new();
    let mut matches = Vec::new();
    let mut candidates_by_path: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
    for (index, diagnostic) in candidate.iter().enumerate() {
        candidates_by_path
            .entry(&diagnostic.path)
            .or_default()
            .push(index);
    }
    for (baseline_index, baseline_diagnostic) in baseline.iter().enumerate() {
        let Some(indices) = candidates_by_path.get(baseline_diagnostic.path.as_str()) else {
            continue;
        };
        let best = indices
            .iter()
            .copied()
            .filter(|index| !candidate_used.contains(index))
            .filter_map(|candidate_index| {
                mappings
                    .confidence_between(baseline_diagnostic, &candidate[candidate_index])
                    .map(|confidence| (candidate_index, confidence))
            })
            .min_by_key(|(candidate_index, confidence)| {
                (
                    position_distance(baseline_diagnostic, &candidate[*candidate_index]),
                    *confidence,
                    *candidate_index,
                )
            });
        if let Some((candidate_index, confidence)) = best {
            baseline_used.insert(baseline_index);
            candidate_used.insert(candidate_index);
            matches.push(DiagnosticMatch {
                kind: if confidence == MappingConfidence::Approximate {
                    MatchKind::ApproximateRuleMatch
                } else {
                    classify_correlated(baseline_diagnostic, &candidate[candidate_index])
                },
                mapping_confidence: Some(confidence),
                baseline: baseline_diagnostic.clone(),
                candidate: candidate[candidate_index].clone(),
            });
        }
    }

    let mut baseline_only = Vec::new();
    let mut unmapped_baseline = Vec::new();
    for (index, diagnostic) in baseline.into_iter().enumerate() {
        if baseline_used.contains(&index) {
            continue;
        }
        let mapped = diagnostic.code.as_deref().is_some_and(|code| {
            mappings
                .confidence_for_rule(baseline_tool, code, candidate_tool)
                .is_some()
        });
        if mapped {
            baseline_only.push(diagnostic);
        } else {
            unmapped_baseline.push(diagnostic);
        }
    }
    let mut candidate_only = Vec::new();
    let mut unmapped_candidate = Vec::new();
    for (index, diagnostic) in candidate.into_iter().enumerate() {
        if candidate_used.contains(&index) {
            continue;
        }
        let mapped = diagnostic.code.as_deref().is_some_and(|code| {
            mappings
                .confidence_for_rule(candidate_tool, code, baseline_tool)
                .is_some()
        });
        if mapped {
            candidate_only.push(diagnostic);
        } else {
            unmapped_candidate.push(diagnostic);
        }
    }
    matches.sort_by(|left, right| match_sort_key(left).cmp(&match_sort_key(right)));
    MatchResult {
        matches,
        baseline_only,
        candidate_only,
        unmapped_baseline,
        unmapped_candidate,
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
        diagnostic.code.as_deref(),
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

pub fn observed_rule_ids(diagnostics: &[Diagnostic]) -> Vec<String> {
    diagnostics
        .iter()
        .filter_map(|diagnostic| diagnostic.code.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

#[cfg(test)]
mod tests {
    use crate::config::RuleMappingConfig;
    use crate::model::{Diagnostic, DiagnosticData, Severity, Span, Tool};

    use super::{
        AliasTable, MappingConfidence, MatchKind, RuleMappingTable, canonical_base,
        compare_with_mappings,
    };

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

    fn mapping(confidence: MappingConfidence) -> RuleMappingConfig {
        RuleMappingConfig {
            baseline_tool: Tool::Eslint,
            baseline: "custom-eslint".into(),
            candidate_tool: Tool::Biome,
            candidate: "lint/custom/customBiome".into(),
            confidence,
            notes: None,
        }
    }

    #[test]
    fn canonicalizes_namespaces_and_case() {
        assert_eq!(canonical_base("lint/suspicious/noDebugger"), "no-debugger");
        assert_eq!(canonical_base("eslint/no_debugger"), "no-debugger");
        assert_eq!(canonical_base("eslint(noDebugger)"), "no-debugger");
    }

    #[test]
    fn explicit_mapping_is_symmetric() {
        let table = RuleMappingTable::new(&[mapping(MappingConfidence::Exact)], &[]);
        let baseline = diagnostic(Tool::Biome, Some("lint/custom/customBiome"), 2);
        let candidate = diagnostic(Tool::Eslint, Some("custom-eslint"), 2);
        assert_eq!(
            table.confidence_between(&baseline, &candidate),
            Some(MappingConfidence::Exact)
        );
    }

    #[test]
    fn approximate_mapping_never_becomes_exact() {
        let table = RuleMappingTable::new(&[mapping(MappingConfidence::Approximate)], &[]);
        let result = compare_with_mappings(
            vec![diagnostic(Tool::Eslint, Some("custom-eslint"), 2)],
            vec![diagnostic(Tool::Biome, Some("lint/custom/customBiome"), 2)],
            &table,
            Tool::Eslint,
            Tool::Biome,
        );
        assert_eq!(result.matches[0].kind, MatchKind::ApproximateRuleMatch);
        assert_eq!(
            result.matches[0].mapping_confidence,
            Some(MappingConfidence::Approximate)
        );
    }

    #[test]
    fn unmapped_is_separate_from_mapped_baseline_only() {
        let table = RuleMappingTable::new(&[mapping(MappingConfidence::Exact)], &[]);
        let result = compare_with_mappings(
            vec![
                diagnostic(Tool::Eslint, Some("custom-eslint"), 2),
                diagnostic(Tool::Eslint, Some("unknown-rule"), 3),
            ],
            Vec::new(),
            &table,
            Tool::Eslint,
            Tool::Biome,
        );
        assert_eq!(result.baseline_only.len(), 1);
        assert_eq!(result.unmapped_baseline.len(), 1);
    }

    #[test]
    fn built_in_exact_match_is_found() {
        let result = compare_with_mappings(
            vec![diagnostic(Tool::Eslint, Some("no-debugger"), 2)],
            vec![diagnostic(
                Tool::Biome,
                Some("lint/suspicious/noDebugger"),
                2,
            )],
            &RuleMappingTable::default(),
            Tool::Eslint,
            Tool::Biome,
        );
        assert_eq!(result.matches[0].kind, MatchKind::ExactMatch);
    }

    #[test]
    fn three_tool_alias_expands_to_deterministic_exact_pairs() {
        let aliases = [crate::config::AliasConfig {
            eslint: Some("eslint-rule".into()),
            biome: Some("lint/custom/biomeRule".into()),
            oxlint: Some("plugin/oxlint-rule".into()),
        }];
        let table = RuleMappingTable::new(&[], &aliases);
        for pair in [
            (Tool::Eslint, Tool::Biome),
            (Tool::Eslint, Tool::Oxlint),
            (Tool::Biome, Tool::Oxlint),
        ] {
            assert_eq!(table.configured_counts(pair.0, pair.1), (1, 0));
        }
    }
}
