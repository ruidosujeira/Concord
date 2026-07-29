use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Instant;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tempfile::Builder;

use crate::adapters::{FormatterAdapter, compare_format_file, run_linter};
use crate::config::Config;
use crate::error::{ConcordError, Result};
use crate::matching::{AliasTable, MatchKind, compare};
use crate::model::{Diagnostic, Severity, Tool};
use crate::process::ProcessRunner;
use crate::report::FormatStatus;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReduceMode {
    Lint,
    Format,
}

#[derive(Debug)]
pub struct ReductionRequest {
    pub mode: ReduceMode,
    pub baseline: Tool,
    pub candidate: Tool,
    pub input: PathBuf,
    pub output: Option<PathBuf>,
    pub mismatch: usize,
    pub timeout_seconds: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MismatchSignature {
    pub side: String,
    pub category: String,
    pub canonical_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub baseline: Option<DiagnosticSignature>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub candidate: Option<DiagnosticSignature>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticSignature {
    pub canonical_code: Option<String>,
    pub message: String,
    pub severity: Severity,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReductionResult {
    pub schema_version: u32,
    pub mode: ReduceMode,
    pub baseline: Tool,
    pub candidate: Tool,
    pub signature: MismatchSignature,
    pub original_lines: usize,
    pub original_bytes: usize,
    pub reduced_lines: usize,
    pub reduced_bytes: usize,
    pub attempts: usize,
    pub duration_ms: u128,
    pub output: String,
}

impl ReductionResult {
    pub fn terminal_summary(&self) -> String {
        format!(
            "Concord reduction\n\nOriginal:  {:>5} lines, {}\nReduced:   {:>5} lines, {}\nAttempts:  {}\nDuration:  {:.1}s\nOutput:    {}",
            self.original_lines,
            human_bytes(self.original_bytes),
            self.reduced_lines,
            human_bytes(self.reduced_bytes),
            self.attempts,
            self.duration_ms as f64 / 1_000.0,
            self.output
        )
    }
}

pub fn reduce(root: &Path, config: &Config, request: ReductionRequest) -> Result<ReductionResult> {
    if !request.input.is_file() {
        return Err(ConcordError::usage(format!(
            "reduction input is not a file: {}",
            request.input.display()
        )));
    }
    let output = request
        .output
        .clone()
        .unwrap_or_else(|| default_output(&request.input));
    if same_path(&request.input, &output) {
        return Err(ConcordError::usage(
            "reduction output must not overwrite the original file",
        ));
    }
    let original = fs::read_to_string(&request.input).map_err(|error| {
        ConcordError::io("failed to read reduction input", &request.input, error)
    })?;
    let parent = request
        .input
        .parent()
        .ok_or_else(|| ConcordError::usage("reduction input must have a parent directory"))?;
    let suffix = request
        .input
        .extension()
        .map(|extension| format!(".{}", extension.to_string_lossy()))
        .unwrap_or_default();
    let temporary = Builder::new()
        .prefix(".concord-reduce-")
        .suffix(&suffix)
        .tempfile_in(parent)
        .map_err(|error| ConcordError::io("failed to create reduction file", parent, error))?;
    let temp_path = temporary.path().to_path_buf();
    fs::write(&temp_path, &original).map_err(|error| {
        ConcordError::io("failed to initialize reduction file", &temp_path, error)
    })?;

    let runner = ProcessRunner::new(root.to_path_buf(), config.clone(), request.timeout_seconds);
    let aliases = AliasTable::new(&config.matching.aliases);
    let predicate = Predicate::new(root, runner, aliases, &request, temp_path)?;
    let signature = predicate.signature.clone();
    let started = Instant::now();
    let original_lines = line_count(&original);
    let original_bytes = original.len();
    let mut cache = PredicateCache::default();
    cache.values.insert(content_hash(&original), true);
    let mut evaluator = |candidate: &str| -> bool {
        cache.evaluate(candidate, |content| predicate.preserves(content))
    };
    let reduced = delta_debug(&original, &mut evaluator);
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            ConcordError::io("failed to create reduction output directory", parent, error)
        })?;
    }
    fs::write(&output, &reduced).map_err(|error| {
        ConcordError::io("failed to write reduced reproduction", &output, error)
    })?;
    let saved = fs::read_to_string(&output)
        .map_err(|error| ConcordError::io("failed to read reduced reproduction", &output, error))?;
    if !predicate.verify(&saved)? {
        return Err(ConcordError::operational(format!(
            "the saved reproduction no longer preserves the selected mismatch\noutput: {}",
            output.display()
        )));
    }
    Ok(ReductionResult {
        schema_version: 1,
        mode: request.mode,
        baseline: request.baseline,
        candidate: request.candidate,
        signature,
        original_lines,
        original_bytes,
        reduced_lines: line_count(&reduced),
        reduced_bytes: reduced.len(),
        attempts: cache.misses,
        duration_ms: started.elapsed().as_millis(),
        output: output.to_string_lossy().into_owned(),
    })
}

struct Predicate<'a> {
    root: &'a Path,
    runner: ProcessRunner,
    aliases: AliasTable,
    mode: ReduceMode,
    baseline: Tool,
    candidate: Tool,
    temp_path: PathBuf,
    signature: MismatchSignature,
    baseline_formatter: Option<FormatterAdapter>,
    candidate_formatter: Option<FormatterAdapter>,
}

impl<'a> Predicate<'a> {
    fn new(
        root: &'a Path,
        runner: ProcessRunner,
        aliases: AliasTable,
        request: &ReductionRequest,
        temp_path: PathBuf,
    ) -> Result<Self> {
        let (baseline_formatter, candidate_formatter) = if request.mode == ReduceMode::Format {
            (
                Some(FormatterAdapter::resolve(&runner, request.baseline)?),
                Some(FormatterAdapter::resolve(&runner, request.candidate)?),
            )
        } else {
            (None, None)
        };
        let mut predicate = Self {
            root,
            runner,
            aliases,
            mode: request.mode,
            baseline: request.baseline,
            candidate: request.candidate,
            temp_path,
            signature: MismatchSignature {
                side: String::new(),
                category: String::new(),
                canonical_code: None,
                baseline: None,
                candidate: None,
            },
            baseline_formatter,
            candidate_formatter,
        };
        let signatures = predicate.current_signatures()?;
        predicate.signature = signatures.get(request.mismatch).cloned().ok_or_else(|| {
            ConcordError::usage(format!(
                "mismatch index {} does not exist; comparison produced {} mismatches",
                request.mismatch,
                signatures.len()
            ))
        })?;
        Ok(predicate)
    }

    fn preserves(&self, content: &str) -> bool {
        self.verify(content).unwrap_or(false)
    }

    fn verify(&self, content: &str) -> Result<bool> {
        fs::write(&self.temp_path, content).map_err(|error| {
            ConcordError::io(
                "failed to write temporary reduction candidate",
                &self.temp_path,
                error,
            )
        })?;
        Ok(self.current_signatures()?.contains(&self.signature))
    }

    fn current_signatures(&self) -> Result<Vec<MismatchSignature>> {
        match self.mode {
            ReduceMode::Lint => self.lint_signatures(),
            ReduceMode::Format => self.format_signatures(),
        }
    }

    fn lint_signatures(&self) -> Result<Vec<MismatchSignature>> {
        let files = [self.temp_path.clone()];
        let (baseline_join, candidate_join) = thread::scope(|scope| {
            let left =
                scope.spawn(|| run_linter(&self.runner, self.baseline, &files, &self.aliases));
            let right =
                scope.spawn(|| run_linter(&self.runner, self.candidate, &files, &self.aliases));
            (left.join(), right.join())
        });
        let baseline = baseline_join
            .map_err(|_| ConcordError::operational("baseline reducer worker panicked"))??;
        let candidate = candidate_join
            .map_err(|_| ConcordError::operational("candidate reducer worker panicked"))??;
        let result = compare(baseline.diagnostics, candidate.diagnostics);
        let mut signatures = Vec::new();
        signatures.extend(
            result
                .baseline_only
                .into_iter()
                .map(|diagnostic| unmatched_signature("baseline", diagnostic)),
        );
        signatures.extend(
            result
                .candidate_only
                .into_iter()
                .map(|diagnostic| unmatched_signature("candidate", diagnostic)),
        );
        signatures.extend(
            result
                .matches
                .into_iter()
                .filter(|item| item.kind != MatchKind::ExactMatch)
                .map(|item| MismatchSignature {
                    side: "both".into(),
                    category: match_kind_name(item.kind).into(),
                    canonical_code: item
                        .baseline
                        .canonical_code
                        .clone()
                        .or(item.candidate.canonical_code.clone()),
                    baseline: Some(diagnostic_signature(&item.baseline)),
                    candidate: Some(diagnostic_signature(&item.candidate)),
                }),
        );
        Ok(signatures)
    }

    fn format_signatures(&self) -> Result<Vec<MismatchSignature>> {
        let baseline = self
            .baseline_formatter
            .as_ref()
            .ok_or_else(|| ConcordError::operational("baseline formatter was not initialized"))?;
        let candidate = self
            .candidate_formatter
            .as_ref()
            .ok_or_else(|| ConcordError::operational("candidate formatter was not initialized"))?;
        let input = fs::read(&self.temp_path).map_err(|error| {
            ConcordError::io(
                "failed to read temporary reduction file",
                &self.temp_path,
                error,
            )
        })?;
        let result = compare_format_file(
            self.root,
            &self.temp_path,
            &input,
            baseline,
            candidate,
            false,
        );
        if matches!(
            result.status,
            FormatStatus::BaselineFailed | FormatStatus::CandidateFailed | FormatStatus::BothFailed
        ) {
            return Err(ConcordError::operational(
                "a formatter failed while evaluating the reduction",
            ));
        }
        if result.status == FormatStatus::Identical {
            return Ok(Vec::new());
        }
        Ok(vec![MismatchSignature {
            side: "both".into(),
            category: format_status_name(result.status).into(),
            canonical_code: None,
            baseline: None,
            candidate: None,
        }])
    }
}

fn unmatched_signature(side: &str, diagnostic: Diagnostic) -> MismatchSignature {
    let canonical_code = diagnostic.canonical_code.clone();
    let identity = diagnostic_signature(&diagnostic);
    let (baseline, candidate) = if side == "baseline" {
        (Some(identity), None)
    } else {
        (None, Some(identity))
    };
    MismatchSignature {
        side: side.into(),
        category: format!("{side}_only"),
        canonical_code,
        baseline,
        candidate,
    }
}

fn diagnostic_signature(diagnostic: &Diagnostic) -> DiagnosticSignature {
    DiagnosticSignature {
        canonical_code: diagnostic.canonical_code.clone(),
        message: diagnostic.message.clone(),
        severity: diagnostic.severity,
    }
}

#[derive(Default)]
struct PredicateCache {
    values: HashMap<String, bool>,
    misses: usize,
}

impl PredicateCache {
    fn evaluate(&mut self, content: &str, predicate: impl FnOnce(&str) -> bool) -> bool {
        let hash = content_hash(content);
        if let Some(value) = self.values.get(&hash) {
            return *value;
        }
        self.misses += 1;
        let value = predicate(content);
        self.values.insert(hash, value);
        value
    }
}

fn delta_debug(source: &str, predicate: &mut impl FnMut(&str) -> bool) -> String {
    let mut lines: Vec<String> = source.split_inclusive('\n').map(str::to_owned).collect();
    if lines.is_empty() {
        lines.push(String::new());
    }
    let mut granularity = 2;
    while lines.len() >= 2 {
        let chunk_size = lines.len().div_ceil(granularity);
        let mut reduced = false;
        let mut start = 0;
        while start < lines.len() {
            let end = (start + chunk_size).min(lines.len());
            let candidate = lines
                .iter()
                .enumerate()
                .filter(|(index, _)| *index < start || *index >= end)
                .map(|(_, line)| line.as_str())
                .collect::<String>();
            if predicate(&candidate) {
                lines.drain(start..end);
                granularity = granularity.saturating_sub(1).max(2);
                reduced = true;
                break;
            }
            start = end;
        }
        if !reduced {
            if granularity >= lines.len() {
                break;
            }
            granularity = (granularity * 2).min(lines.len());
        }
    }
    let mut index = 0;
    while index < lines.len() {
        let candidate = lines
            .iter()
            .enumerate()
            .filter(|(line_index, _)| *line_index != index)
            .map(|(_, line)| line.as_str())
            .collect::<String>();
        if predicate(&candidate) {
            lines.remove(index);
        } else {
            index += 1;
        }
    }
    lines.concat()
}

fn content_hash(content: &str) -> String {
    format!("{:x}", Sha256::digest(content.as_bytes()))
}

fn default_output(input: &Path) -> PathBuf {
    let stem = input
        .file_stem()
        .map_or_else(|| "repro".into(), |stem| stem.to_string_lossy());
    let extension = input.extension().map(|value| value.to_string_lossy());
    let name = extension.map_or_else(
        || format!("{stem}.reduced"),
        |extension| format!("{stem}.reduced.{extension}"),
    );
    input.with_file_name(name)
}

fn same_path(left: &Path, right: &Path) -> bool {
    left.canonicalize().ok() == right.canonicalize().ok()
        || (left.parent() == right.parent() && left.file_name() == right.file_name())
}

fn line_count(content: &str) -> usize {
    if content.is_empty() {
        0
    } else {
        content.lines().count()
    }
}

fn human_bytes(bytes: usize) -> String {
    if bytes < 1_024 {
        format!("{bytes} B")
    } else {
        format!("{:.1} KiB", bytes as f64 / 1_024.0)
    }
}

fn match_kind_name(kind: MatchKind) -> &'static str {
    match kind {
        MatchKind::ExactMatch => "exact_match",
        MatchKind::ProbableMatch => "probable_match",
        MatchKind::SeverityChanged => "severity_changed",
        MatchKind::RangeChanged => "range_changed",
        MatchKind::MessageChanged => "message_changed",
    }
}

fn format_status_name(status: FormatStatus) -> &'static str {
    match status {
        FormatStatus::Identical => "identical",
        FormatStatus::Different => "different",
        FormatStatus::BaselineNonIdempotent => "baseline_non_idempotent",
        FormatStatus::CandidateNonIdempotent => "candidate_non_idempotent",
        FormatStatus::BaselineFailed => "baseline_failed",
        FormatStatus::CandidateFailed => "candidate_failed",
        FormatStatus::BothFailed => "both_failed",
    }
}

#[cfg(test)]
mod tests {
    use crate::model::{Diagnostic, DiagnosticData, Severity, Tool};

    use super::{PredicateCache, delta_debug, unmatched_signature};

    #[test]
    fn cache_does_not_repeat_predicate_calls() {
        let mut cache = PredicateCache::default();
        let mut calls = 0;
        assert!(cache.evaluate("same", |_| {
            calls += 1;
            true
        }));
        assert!(cache.evaluate("same", |_| {
            calls += 1;
            false
        }));
        assert_eq!(calls, 1);
        assert_eq!(cache.misses, 1);
    }

    #[test]
    fn textual_reducer_keeps_required_line() {
        let source = "one\ntwo\nKEEP\nthree\nfour\n";
        let reduced = delta_debug(source, &mut |candidate| candidate.contains("KEEP"));
        assert_eq!(reduced, "KEEP\n");
    }

    #[test]
    fn mismatch_signature_rejects_another_diagnostic_with_the_same_rule() {
        let diagnostic = |message: &str| {
            Diagnostic::new(
                Tool::Eslint,
                "temporary.ts",
                DiagnosticData {
                    code: Some("no-unused-vars".into()),
                    canonical_code: Some("no-unused-vars".into()),
                    severity: Severity::Warning,
                    message: message.into(),
                    span: None,
                    fix: None,
                },
            )
        };
        let selected =
            unmatched_signature("baseline", diagnostic("This variable error is unused."));
        let drifted = unmatched_signature(
            "baseline",
            diagnostic("This variable userTryingToGet is unused."),
        );

        assert_ne!(selected, drifted);
        assert!(![drifted].contains(&selected));
    }
}
