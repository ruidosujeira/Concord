//! The existing single-project pipeline, run against one corpus entry.
//!
//! This composes the same primitives `concord compare` uses — discovery, the
//! comparison plan, the linter and formatter adapters, and the matcher — and
//! keeps the existing definition of what counts as a divergence. Nothing here
//! decides that question itself.

use std::collections::BTreeMap;
use std::fs;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::adapters::{FormatterAdapter, compare_format_file, run_linter};
use crate::capabilities::{CapabilityCatalog, ComparisonPlan, ToolAvailability};
use crate::config::Config;
use crate::discovery::discover_excluding;
use crate::matching::{AliasTable, MatchResult, RuleMappingTable, compare_with_mappings};
use crate::model::{Diagnostic, Tool};
use crate::process::ProcessRunner;
use crate::reduce::{ReduceMode, format_mismatch_signature, lint_mismatch_signatures};
use crate::report::{ComparisonProfile, FormatComparisonStatus};

use super::entry::{EntryError, EntryErrorKind};
use super::finding::{Finding, FindingContext};

/// Everything an entry analysis needs that does not vary between entries.
#[derive(Debug, Clone)]
pub struct AnalysisContext {
    /// The invocation's configuration, with the two tool commands pinned to
    /// absolute paths so that no executable inside an untrusted entry is ever
    /// resolved or run.
    pub config: Config,
    pub capabilities: CapabilityCatalog,
    pub finding: FindingContext,
    pub profile: ComparisonProfile,
    pub max_files: usize,
    pub timeout: Duration,
    pub normalize_eol: bool,
}

#[derive(Debug, Clone, Default)]
pub struct EntryAnalysis {
    pub findings: Vec<Finding>,
    pub files_discovered: usize,
    pub files_analyzed: usize,
    pub truncated: bool,
}

/// Analyze one project. Every failure, including a panic, is returned as an
/// entry error rather than propagated.
pub fn analyze(
    context: &AnalysisContext,
    project: &str,
    root: &Path,
) -> std::result::Result<EntryAnalysis, EntryError> {
    let deadline = Instant::now() + context.timeout;
    let outcome = catch_unwind(AssertUnwindSafe(|| run(context, project, root, deadline)));
    match outcome {
        Ok(result) => result,
        Err(_) => Err(EntryError::new(
            EntryErrorKind::Panic,
            "a worker panicked while analyzing this entry",
        )),
    }
}

fn run(
    context: &AnalysisContext,
    project: &str,
    root: &Path,
    deadline: Instant,
) -> std::result::Result<EntryAnalysis, EntryError> {
    let discovered = discover_excluding(root, &[], &context.config.discovery, &[])
        .map_err(|error| EntryError::new(EntryErrorKind::Unreadable, error.to_string()))?;
    if discovered.is_empty() {
        return Err(EntryError::new(
            EntryErrorKind::NoAnalyzableFiles,
            "no file matches the configured discovery patterns",
        ));
    }
    let files_discovered = discovered.len();
    let truncated = files_discovered > context.max_files;
    let files: Vec<PathBuf> = discovered.into_iter().take(context.max_files).collect();
    let files_analyzed = files.len();
    check_deadline(deadline)?;

    let findings = match context.finding.mode {
        ReduceMode::Lint => lint(context, project, root, &files, deadline)?,
        ReduceMode::Format => format(context, project, root, &files, deadline)?,
    };
    Ok(EntryAnalysis {
        findings,
        files_discovered,
        files_analyzed,
        truncated,
    })
}

fn lint(
    context: &AnalysisContext,
    project: &str,
    root: &Path,
    files: &[PathBuf],
    deadline: Instant,
) -> std::result::Result<Vec<Finding>, EntryError> {
    let baseline_tool = context.finding.baseline;
    let candidate_tool = context.finding.candidate;
    let runner = entry_runner(context, root, deadline)?;
    let plan = ComparisonPlan::build(
        root,
        files,
        baseline_tool,
        candidate_tool,
        None,
        None,
        &context.capabilities,
        vec![
            availability(&runner, baseline_tool),
            availability(&runner, candidate_tool),
        ],
    );
    let comparable = plan.comparable_paths(root);
    if comparable.is_empty() {
        return Err(EntryError::new(
            EntryErrorKind::NoAnalyzableFiles,
            "no discovered file is comparable with both tools",
        ));
    }
    let aliases = AliasTable::new(&context.config.matching.aliases);
    let mappings = RuleMappingTable::new(
        &context.config.matching.rules,
        &context.config.matching.aliases,
    );
    // The two linters run one after the other so that an entry never exceeds
    // the concurrency the corpus scheduler already grants it.
    let baseline =
        run_linter(&runner, baseline_tool, &comparable, &aliases).map_err(tool_failure)?;
    check_deadline(deadline)?;
    let candidate_runner = entry_runner(context, root, deadline)?;
    let candidate = run_linter(&candidate_runner, candidate_tool, &comparable, &aliases)
        .map_err(tool_failure)?;
    check_deadline(deadline)?;

    let result = compare_with_mappings(
        baseline.diagnostics,
        candidate.diagnostics,
        &mappings,
        baseline_tool,
        candidate_tool,
    );
    let mut sizes = SizeCache::new(root);
    let mut findings = Vec::new();
    for (path, per_file) in partition_by_path(result) {
        for signature in lint_mismatch_signatures(per_file) {
            if !is_divergence(
                &signature.category,
                context.profile,
                context.config.matching.count_probable_as_match,
            ) {
                continue;
            }
            findings.push(Finding::lint(
                context.finding,
                signature,
                project,
                &path,
                sizes.get(&path),
            ));
        }
    }
    Ok(findings)
}

fn format(
    context: &AnalysisContext,
    project: &str,
    root: &Path,
    files: &[PathBuf],
    deadline: Instant,
) -> std::result::Result<Vec<Finding>, EntryError> {
    let baseline_tool = context.finding.baseline;
    let candidate_tool = context.finding.candidate;
    let runner = entry_runner(context, root, deadline)?;
    let baseline = FormatterAdapter::resolve(&runner, baseline_tool).map_err(tool_failure)?;
    let candidate = FormatterAdapter::resolve(&runner, candidate_tool).map_err(tool_failure)?;
    let plan = ComparisonPlan::build(
        root,
        files,
        baseline_tool,
        candidate_tool,
        Some(baseline.version()),
        Some(candidate.version()),
        &context.capabilities,
        vec![
            availability(&runner, baseline_tool),
            availability(&runner, candidate_tool),
        ],
    );
    let comparable = plan.comparable_paths(root);
    if comparable.is_empty() {
        return Err(EntryError::new(
            EntryErrorKind::NoAnalyzableFiles,
            "no discovered file is comparable with both tools",
        ));
    }
    let mut sizes = SizeCache::new(root);
    let mut findings = Vec::new();
    for path in comparable {
        check_deadline(deadline)?;
        let input = fs::read(&path).map_err(|error| {
            EntryError::new(
                EntryErrorKind::Unreadable,
                format!("failed to read {}: {error}", path.display()),
            )
        })?;
        let result = compare_format_file(
            root,
            &path,
            &input,
            &baseline,
            &candidate,
            context.normalize_eol,
        );
        if result.status == FormatComparisonStatus::Failed {
            return Err(formatter_failure(&result));
        }
        let Some(signature) = format_mismatch_signature(result.status) else {
            continue;
        };
        if !is_format_divergence(result.status) {
            continue;
        }
        let bytes = sizes.get(&result.path);
        findings.push(Finding::format(
            context.finding,
            signature,
            result.diff.as_deref(),
            project,
            &result.path,
            bytes,
        ));
    }
    Ok(findings)
}

/// Mirrors `LintReport::has_differences`: the corpus never redefines what a
/// divergence is.
fn is_divergence(
    category: &str,
    profile: ComparisonProfile,
    count_probable_as_match: bool,
) -> bool {
    match category {
        "baseline_only" | "candidate_only" | "severity_changed" | "range_changed"
        | "message_changed" => true,
        "unmapped_baseline" | "unmapped_candidate" => profile == ComparisonProfile::Raw,
        "probable_match" => !count_probable_as_match,
        _ => false,
    }
}

/// Mirrors `FormatReport::has_differences`.
fn is_format_divergence(status: FormatComparisonStatus) -> bool {
    matches!(
        status,
        FormatComparisonStatus::Different
            | FormatComparisonStatus::BaselineNonIdempotent
            | FormatComparisonStatus::CandidateNonIdempotent
            | FormatComparisonStatus::BothNonIdempotent
    )
}

/// Split a comparison result per file, so that each occurrence carries the
/// file it was observed in and the reducer indexes it the same way.
fn partition_by_path(result: MatchResult) -> BTreeMap<String, MatchResult> {
    let mut buckets: BTreeMap<String, MatchResult> = BTreeMap::new();
    let empty = || MatchResult {
        matches: Vec::new(),
        baseline_only: Vec::new(),
        candidate_only: Vec::new(),
        unmapped_baseline: Vec::new(),
        unmapped_candidate: Vec::new(),
    };
    let push = |buckets: &mut BTreeMap<String, MatchResult>,
                diagnostic: Diagnostic,
                select: fn(&mut MatchResult) -> &mut Vec<Diagnostic>| {
        let bucket = buckets.entry(diagnostic.path.clone()).or_insert_with(empty);
        select(bucket).push(diagnostic);
    };
    for diagnostic in result.baseline_only {
        push(&mut buckets, diagnostic, |bucket| &mut bucket.baseline_only);
    }
    for diagnostic in result.candidate_only {
        push(&mut buckets, diagnostic, |bucket| {
            &mut bucket.candidate_only
        });
    }
    for diagnostic in result.unmapped_baseline {
        push(&mut buckets, diagnostic, |bucket| {
            &mut bucket.unmapped_baseline
        });
    }
    for diagnostic in result.unmapped_candidate {
        push(&mut buckets, diagnostic, |bucket| {
            &mut bucket.unmapped_candidate
        });
    }
    for item in result.matches {
        buckets
            .entry(item.baseline.path.clone())
            .or_insert_with(empty)
            .matches
            .push(item);
    }
    buckets
}

/// File sizes for representative selection, read once per path.
struct SizeCache<'a> {
    root: &'a Path,
    sizes: BTreeMap<String, u64>,
}

impl<'a> SizeCache<'a> {
    fn new(root: &'a Path) -> Self {
        Self {
            root,
            sizes: BTreeMap::new(),
        }
    }

    fn get(&mut self, relative: &str) -> u64 {
        if let Some(size) = self.sizes.get(relative) {
            return *size;
        }
        let size = fs::metadata(crate::corpus::project_file(self.root, relative))
            .map(|metadata| metadata.len())
            .unwrap_or(u64::MAX);
        self.sizes.insert(relative.to_owned(), size);
        size
    }
}

fn entry_runner(
    context: &AnalysisContext,
    root: &Path,
    deadline: Instant,
) -> std::result::Result<ProcessRunner, EntryError> {
    let remaining = deadline.saturating_duration_since(Instant::now()).as_secs();
    if remaining == 0 {
        return Err(timeout());
    }
    Ok(ProcessRunner::new(
        root.to_path_buf(),
        context.config.clone(),
        Some(remaining.min(context.config.execution.timeout_seconds.max(1))),
    ))
}

fn availability(runner: &ProcessRunner, tool: Tool) -> ToolAvailability {
    match runner.resolve(tool) {
        Ok(resolved) => ToolAvailability {
            tool,
            available: true,
            executable: Some(resolved.executable.to_string_lossy().into_owned()),
            reason: None,
        },
        Err(error) => ToolAvailability {
            tool,
            available: false,
            executable: None,
            reason: Some(error.to_string()),
        },
    }
}

fn check_deadline(deadline: Instant) -> std::result::Result<(), EntryError> {
    if Instant::now() >= deadline {
        Err(timeout())
    } else {
        Ok(())
    }
}

fn timeout() -> EntryError {
    EntryError::new(
        EntryErrorKind::Timeout,
        "the entry exceeded its analysis timeout and was abandoned",
    )
}

fn tool_failure(error: crate::error::ConcordError) -> EntryError {
    classify_tool_failure(&error.to_string())
}

fn formatter_failure(result: &crate::report::FormatFileResult) -> EntryError {
    let detail = [&result.baseline, &result.candidate]
        .into_iter()
        .filter_map(|outcome| {
            outcome
                .error
                .as_ref()
                .map(|error| format!("{} {}: {error}", result.path, outcome.tool))
        })
        .collect::<Vec<_>>()
        .join("\n");
    classify_tool_failure(&detail)
}

fn classify_tool_failure(message: &str) -> EntryError {
    let kind = if message.contains("timed out") {
        EntryErrorKind::Timeout
    } else if message.contains("not UTF-8") {
        EntryErrorKind::NotUtf8
    } else {
        EntryErrorKind::ToolFailure
    };
    EntryError::new(kind, message.to_owned())
}

#[cfg(test)]
mod tests {
    use super::{is_divergence, is_format_divergence, partition_by_path};
    use crate::matching::{DiagnosticMatch, MappingConfidence, MatchKind, MatchResult};
    use crate::model::{Diagnostic, DiagnosticData, Severity, Tool};
    use crate::report::{ComparisonProfile, FormatComparisonStatus};

    fn diagnostic(path: &str) -> Diagnostic {
        Diagnostic::new(
            Tool::Eslint,
            path,
            DiagnosticData {
                code: Some("no-debugger".into()),
                canonical_code: Some("no-debugger".into()),
                severity: Severity::Error,
                message: "message".into(),
                span: None,
                fix: None,
            },
        )
    }

    #[test]
    fn divergence_categories_match_the_existing_definition() {
        for category in [
            "baseline_only",
            "candidate_only",
            "severity_changed",
            "range_changed",
            "message_changed",
        ] {
            assert!(is_divergence(category, ComparisonProfile::Raw, false));
            assert!(is_divergence(
                category,
                ComparisonProfile::Comparable,
                false
            ));
        }
        for category in ["unmapped_baseline", "unmapped_candidate"] {
            assert!(is_divergence(category, ComparisonProfile::Raw, false));
            assert!(!is_divergence(
                category,
                ComparisonProfile::Comparable,
                false
            ));
        }
        assert!(is_divergence(
            "probable_match",
            ComparisonProfile::Raw,
            false
        ));
        assert!(!is_divergence(
            "probable_match",
            ComparisonProfile::Raw,
            true
        ));
        assert!(!is_divergence(
            "approximate_rule_match",
            ComparisonProfile::Raw,
            false
        ));
        assert!(!is_divergence("exact_match", ComparisonProfile::Raw, false));
    }

    #[test]
    fn format_divergences_exclude_identical_unsupported_and_skipped() {
        assert!(is_format_divergence(FormatComparisonStatus::Different));
        assert!(is_format_divergence(
            FormatComparisonStatus::BothNonIdempotent
        ));
        for status in [
            FormatComparisonStatus::Identical,
            FormatComparisonStatus::Unsupported,
            FormatComparisonStatus::Skipped,
        ] {
            assert!(!is_format_divergence(status));
        }
    }

    #[test]
    fn results_partition_per_file_without_losing_occurrences() {
        let result = MatchResult {
            matches: vec![DiagnosticMatch {
                kind: MatchKind::RangeChanged,
                mapping_confidence: Some(MappingConfidence::Exact),
                baseline: diagnostic("src/a.ts"),
                candidate: diagnostic("src/a.ts"),
            }],
            baseline_only: vec![diagnostic("src/a.ts"), diagnostic("src/b.ts")],
            candidate_only: vec![diagnostic("src/b.ts")],
            unmapped_baseline: Vec::new(),
            unmapped_candidate: vec![diagnostic("src/c.ts")],
        };
        let buckets = partition_by_path(result);
        assert_eq!(
            buckets.keys().collect::<Vec<_>>(),
            ["src/a.ts", "src/b.ts", "src/c.ts"]
        );
        assert_eq!(buckets["src/a.ts"].baseline_only.len(), 1);
        assert_eq!(buckets["src/a.ts"].matches.len(), 1);
        assert_eq!(buckets["src/b.ts"].baseline_only.len(), 1);
        assert_eq!(buckets["src/b.ts"].candidate_only.len(), 1);
        assert_eq!(buckets["src/c.ts"].unmapped_candidate.len(), 1);
    }
}
