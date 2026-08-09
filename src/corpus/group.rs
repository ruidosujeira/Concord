//! Deduplication of findings across projects.

use std::collections::BTreeMap;

use super::finding::Finding;
use super::reduction::GroupReduction;

/// Findings that share a fingerprint, with one representative occurrence.
#[derive(Debug, Clone)]
pub struct FindingGroup {
    pub fingerprint: String,
    pub category: String,
    pub rule: Option<String>,
    pub occurrences: usize,
    /// Sorted, deduplicated project identities in manifest form.
    pub projects: Vec<String>,
    pub representative: Finding,
    pub reduction: Option<GroupReduction>,
}

impl FindingGroup {
    pub fn project_count(&self) -> usize {
        self.projects.len()
    }
}

/// Collapse findings into groups and order them: affected project count
/// descending, then occurrence count descending, then fingerprint ascending.
/// The result does not depend on the order of `findings`.
pub fn group(findings: Vec<Finding>) -> Vec<FindingGroup> {
    let mut buckets: BTreeMap<String, Vec<Finding>> = BTreeMap::new();
    for finding in findings {
        buckets
            .entry(finding.fingerprint.clone())
            .or_default()
            .push(finding);
    }
    let mut groups: Vec<FindingGroup> = buckets
        .into_iter()
        .filter_map(|(fingerprint, occurrences)| build(fingerprint, occurrences))
        .collect();
    groups.sort_by(|left, right| {
        right
            .project_count()
            .cmp(&left.project_count())
            .then_with(|| right.occurrences.cmp(&left.occurrences))
            .then_with(|| left.fingerprint.cmp(&right.fingerprint))
    });
    groups
}

fn build(fingerprint: String, occurrences: Vec<Finding>) -> Option<FindingGroup> {
    // Smallest source file, then lexicographically smallest project identity,
    // then lexicographically smallest relative file path.
    let representative = occurrences
        .iter()
        .min_by(|left, right| {
            left.file_bytes
                .cmp(&right.file_bytes)
                .then_with(|| left.project.cmp(&right.project))
                .then_with(|| left.path.cmp(&right.path))
        })?
        .clone();
    let mut projects: Vec<String> = occurrences
        .iter()
        .map(|finding| finding.project.clone())
        .collect();
    projects.sort();
    projects.dedup();
    Some(FindingGroup {
        fingerprint,
        category: representative.category.clone(),
        rule: representative.rule.clone(),
        occurrences: occurrences.len(),
        projects,
        representative,
        reduction: None,
    })
}

#[cfg(test)]
mod tests {
    use super::group;
    use crate::corpus::finding::tests::lint_finding;

    fn corpus() -> Vec<super::Finding> {
        vec![
            lint_finding(
                "baseline_only",
                Some("no-debugger"),
                "m",
                "path:b",
                "z.ts",
                90,
            ),
            lint_finding(
                "baseline_only",
                Some("no-debugger"),
                "m",
                "path:a",
                "a.ts",
                40,
            ),
            lint_finding(
                "baseline_only",
                Some("no-debugger"),
                "m",
                "path:a",
                "b.ts",
                40,
            ),
            lint_finding(
                "candidate_only",
                Some("no-console"),
                "m",
                "path:c",
                "c.ts",
                10,
            ),
        ]
    }

    #[test]
    fn the_same_divergence_in_two_projects_forms_one_group() {
        let groups = group(corpus());
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].projects, ["path:a", "path:b"]);
        assert_eq!(groups[0].occurrences, 3);
        assert_eq!(groups[0].category, "baseline_only");
    }

    #[test]
    fn different_divergences_stay_separate() {
        let groups = group(corpus());
        assert_ne!(groups[0].fingerprint, groups[1].fingerprint);
        assert_eq!(groups[1].projects, ["path:c"]);
        assert_eq!(groups[1].occurrences, 1);
    }

    #[test]
    fn representative_and_order_are_stable_under_shuffled_input() {
        let expected = group(corpus());
        // Every rotation and the reverse must produce the same result.
        for rotation in 0..corpus().len() {
            let mut shuffled = corpus();
            shuffled.rotate_left(rotation);
            let actual = group(shuffled);
            assert_eq!(
                actual
                    .iter()
                    .map(|item| (
                        item.fingerprint.clone(),
                        item.representative.project.clone(),
                        item.representative.path.clone()
                    ))
                    .collect::<Vec<_>>(),
                expected
                    .iter()
                    .map(|item| (
                        item.fingerprint.clone(),
                        item.representative.project.clone(),
                        item.representative.path.clone()
                    ))
                    .collect::<Vec<_>>()
            );
        }
        let mut reversed = corpus();
        reversed.reverse();
        assert_eq!(
            group(reversed)[0].representative.path,
            expected[0].representative.path
        );
    }

    #[test]
    fn representative_is_the_smallest_file_then_project_then_path() {
        // Two occurrences share the smallest byte length; the project and then
        // the path break the tie.
        let groups = group(corpus());
        assert_eq!(groups[0].representative.project, "path:a");
        assert_eq!(groups[0].representative.path, "a.ts");
        assert_eq!(groups[0].representative.file_bytes, 40);
    }

    #[test]
    fn groups_order_by_projects_then_occurrences_then_fingerprint() {
        let mut findings = corpus();
        // A second single-project group with more occurrences than the first.
        findings.push(lint_finding(
            "candidate_only",
            Some("no-console"),
            "m",
            "path:c",
            "d.ts",
            10,
        ));
        findings.push(lint_finding(
            "severity_changed",
            Some("no-undef"),
            "m",
            "path:d",
            "e.ts",
            10,
        ));
        let groups = group(findings);
        assert_eq!(groups[0].project_count(), 2);
        assert_eq!(
            groups
                .iter()
                .map(|item| (item.project_count(), item.occurrences))
                .collect::<Vec<_>>(),
            [(2, 3), (1, 2), (1, 1)]
        );
    }
}
