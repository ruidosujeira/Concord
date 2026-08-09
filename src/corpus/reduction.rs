//! Optional reduction of one representative occurrence per finding group.
//!
//! The existing reducer writes its working copy next to the file it is
//! minimizing. Corpus entries are untrusted and local `path:` entries are
//! read-only, so the representative file is copied into a scratch directory
//! first and reduced there. The project root is still passed to the reducer so
//! that tools resolve and execute exactly as they do for a single-project run.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::model::Tool;
use crate::reduce::{MismatchSignature, ReduceMode, ReductionRequest, mismatch_signatures, reduce};

use super::group::FindingGroup;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupReduction {
    pub completed: bool,
    /// Why reduction did not complete. Absent when it did.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// The minimized reproduction. Absent when reduction did not complete.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    pub original_bytes: usize,
    pub reduced_bytes: usize,
    pub attempts: usize,
}

impl GroupReduction {
    pub fn failed(reason: impl Into<String>) -> Self {
        Self {
            completed: false,
            reason: Some(reason.into()),
            source: None,
            original_bytes: 0,
            reduced_bytes: 0,
            attempts: 0,
        }
    }
}

/// One representative occurrence to minimize.
#[derive(Debug)]
pub struct ReductionTask<'a> {
    /// The project root, used only to resolve and run the tools.
    pub project_root: &'a Path,
    /// The representative source file, inside `project_root`.
    pub file: PathBuf,
    /// The mismatch identity that must survive reduction.
    pub signature: &'a MismatchSignature,
    pub timeout: Duration,
}

/// The seam between grouping and the reducer, so that reduction can be
/// observed and substituted in tests.
pub trait GroupReducer: Sync {
    fn reduce(&self, task: &ReductionTask<'_>) -> GroupReduction;
}

/// Drives the existing reducer.
pub struct PipelineReducer {
    config: Config,
    mode: ReduceMode,
    baseline: Tool,
    candidate: Tool,
}

impl PipelineReducer {
    pub fn new(config: Config, mode: ReduceMode, baseline: Tool, candidate: Tool) -> Self {
        Self {
            config,
            mode,
            baseline,
            candidate,
        }
    }

    fn run(&self, task: &ReductionTask<'_>) -> std::result::Result<GroupReduction, String> {
        let deadline = Instant::now() + task.timeout;
        let scratch = tempfile::Builder::new()
            .prefix("concord-corpus-reduce-")
            .tempdir()
            .map_err(|error| format!("failed to create a reduction scratch directory: {error}"))?;
        let name = task
            .file
            .file_name()
            .ok_or_else(|| "the representative occurrence has no file name".to_owned())?;
        let working = scratch.path().join(name);
        fs::copy(&task.file, &working).map_err(|error| {
            format!(
                "failed to copy the representative occurrence: {error} ({})",
                task.file.display()
            )
        })?;

        let signatures = mismatch_signatures(
            task.project_root,
            &self.config,
            self.mode,
            self.baseline,
            self.candidate,
            &working,
            Some(self.remaining(deadline)),
        )
        .map_err(|error| format!("failed to locate the mismatch: {error}"))?;
        let index = signatures
            .iter()
            .position(|candidate| candidate == task.signature)
            .ok_or_else(|| {
                "the representative occurrence no longer reproduces in isolation".to_owned()
            })?;

        let output = scratch.path().join(format!(
            "{}.reduced",
            name.to_string_lossy().replace('/', "_")
        ));
        let result = reduce(
            task.project_root,
            &self.config,
            ReductionRequest {
                mode: self.mode,
                baseline: self.baseline,
                candidate: self.candidate,
                input: working,
                output: Some(output.clone()),
                mismatch: index,
                timeout_seconds: Some(self.remaining(deadline)),
                deadline: Some(deadline),
            },
        )
        .map_err(|error| error.to_string())?;
        if Instant::now() >= deadline {
            return Err(format!(
                "reduction timed out after {}s",
                task.timeout.as_secs()
            ));
        }
        let source = fs::read_to_string(&output)
            .map_err(|error| format!("failed to read the reduced reproduction: {error}"))?;
        Ok(GroupReduction {
            completed: true,
            reason: None,
            source: Some(source),
            original_bytes: result.original_bytes,
            reduced_bytes: result.reduced_bytes,
            attempts: result.attempts,
        })
    }

    fn remaining(&self, deadline: Instant) -> u64 {
        let remaining = deadline
            .saturating_duration_since(Instant::now())
            .as_secs()
            .max(1);
        remaining.min(self.config.execution.timeout_seconds.max(1))
    }
}

impl GroupReducer for PipelineReducer {
    fn reduce(&self, task: &ReductionTask<'_>) -> GroupReduction {
        match self.run(task) {
            Ok(reduction) => reduction,
            Err(reason) => GroupReduction::failed(reason),
        }
    }
}

/// Select the groups to reduce: the `top` largest by affected project count,
/// with ties broken by fingerprint. `None` selects every group.
pub fn selected_indices(groups: &[FindingGroup], top: Option<usize>) -> Vec<usize> {
    let mut order: Vec<usize> = (0..groups.len()).collect();
    order.sort_by(|left, right| {
        groups[*right]
            .project_count()
            .cmp(&groups[*left].project_count())
            .then_with(|| groups[*left].fingerprint.cmp(&groups[*right].fingerprint))
    });
    order.truncate(top.unwrap_or(groups.len()));
    order.sort_unstable();
    order
}

/// Reduce exactly one representative occurrence per selected group.
pub fn reduce_groups(
    groups: &mut [FindingGroup],
    reducer: &dyn GroupReducer,
    roots: &BTreeMap<String, PathBuf>,
    top: Option<usize>,
    timeout: Duration,
    mut each: impl FnMut(&FindingGroup),
) {
    for index in selected_indices(groups, top) {
        let Some(group) = groups.get(index) else {
            continue;
        };
        let representative = &group.representative;
        let reduction = match roots.get(&representative.project) {
            Some(root) => {
                let task = ReductionTask {
                    project_root: root,
                    file: super::project_file(root, &representative.path),
                    signature: &representative.signature,
                    timeout,
                };
                reducer.reduce(&task)
            }
            None => GroupReduction::failed(
                "the project holding the representative occurrence is no longer available",
            ),
        };
        if let Some(group) = groups.get_mut(index) {
            group.reduction = Some(reduction);
            each(group);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::PathBuf;
    use std::sync::Mutex;
    use std::time::Duration;

    use super::{GroupReducer, GroupReduction, ReductionTask, reduce_groups, selected_indices};
    use crate::corpus::finding::tests::lint_finding;
    use crate::corpus::group::group;

    #[derive(Default)]
    struct CountingReducer {
        calls: Mutex<Vec<String>>,
        outcome: Option<GroupReduction>,
    }

    impl GroupReducer for CountingReducer {
        fn reduce(&self, task: &ReductionTask<'_>) -> GroupReduction {
            if let Ok(mut calls) = self.calls.lock() {
                calls.push(task.file.to_string_lossy().into_owned());
            }
            self.outcome.clone().unwrap_or(GroupReduction {
                completed: true,
                reason: None,
                source: Some("reduced\n".into()),
                original_bytes: 10,
                reduced_bytes: 8,
                attempts: 3,
            })
        }
    }

    fn corpus() -> Vec<crate::corpus::finding::Finding> {
        vec![
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
                "path:b",
                "b.ts",
                90,
            ),
            lint_finding(
                "candidate_only",
                Some("no-console"),
                "m",
                "path:a",
                "c.ts",
                10,
            ),
            lint_finding(
                "severity_changed",
                Some("no-undef"),
                "m",
                "path:a",
                "d.ts",
                10,
            ),
        ]
    }

    fn roots() -> BTreeMap<String, PathBuf> {
        [
            ("path:a".to_owned(), PathBuf::from("/projects/a")),
            ("path:b".to_owned(), PathBuf::from("/projects/b")),
        ]
        .into_iter()
        .collect()
    }

    #[test]
    fn the_reducer_runs_once_per_group_not_once_per_occurrence() {
        let mut groups = group(corpus());
        assert_eq!(groups.len(), 3);
        assert_eq!(groups[0].occurrences, 2);
        let reducer = CountingReducer::default();
        reduce_groups(
            &mut groups,
            &reducer,
            &roots(),
            None,
            Duration::from_secs(120),
            |_| {},
        );
        let calls = reducer.calls.lock().expect("call log");
        assert_eq!(calls.len(), 3, "one invocation per group");
        assert!(calls.iter().all(|call| call.contains("/projects/a")));
        assert!(groups.iter().all(|item| {
            item.reduction
                .as_ref()
                .is_some_and(|reduction| reduction.completed)
        }));
    }

    #[test]
    fn reduce_top_limits_by_project_count_with_fingerprint_tie_breaking() {
        let groups = group(corpus());
        let selected = selected_indices(&groups, Some(2));
        assert_eq!(selected.len(), 2);
        assert!(
            selected.contains(&0),
            "the two-project group is always selected"
        );
        let single: Vec<_> = groups
            .iter()
            .enumerate()
            .filter(|(_, item)| item.project_count() == 1)
            .collect();
        let expected = single
            .iter()
            .min_by_key(|(_, item)| item.fingerprint.clone())
            .map(|(index, _)| *index);
        assert_eq!(expected.map(|index| selected.contains(&index)), Some(true));
        assert_eq!(selected_indices(&groups, Some(0)).len(), 0);
        assert_eq!(selected_indices(&groups, None).len(), 3);
    }

    #[test]
    fn a_failed_reduction_keeps_the_unreduced_representative_and_records_why() {
        let mut groups = group(corpus());
        let reducer = CountingReducer {
            calls: Mutex::default(),
            outcome: Some(GroupReduction::failed("reduction timed out after 120s")),
        };
        reduce_groups(
            &mut groups,
            &reducer,
            &roots(),
            None,
            Duration::from_secs(120),
            |_| {},
        );
        let reduction = groups[0].reduction.as_ref().expect("recorded reduction");
        assert!(!reduction.completed);
        assert_eq!(
            reduction.reason.as_deref(),
            Some("reduction timed out after 120s")
        );
        assert!(reduction.source.is_none());
        assert_eq!(groups[0].representative.path, "a.ts");
    }

    #[test]
    fn a_missing_project_root_is_recorded_rather_than_panicking() {
        let mut groups = group(corpus());
        let reducer = CountingReducer::default();
        reduce_groups(
            &mut groups,
            &reducer,
            &BTreeMap::new(),
            None,
            Duration::from_secs(1),
            |_| {},
        );
        assert!(groups.iter().all(|item| {
            item.reduction
                .as_ref()
                .is_some_and(|reduction| !reduction.completed)
        }));
        assert!(reducer.calls.lock().expect("call log").is_empty());
    }
}
