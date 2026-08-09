//! Run the existing differential pipeline over many projects in one pass.
//!
//! A manifest declares fully pinned projects. They are acquired first, then
//! analyzed with bounded parallelism and per-entry isolation, and the findings
//! are fingerprinted, deduplicated across projects and ranked by how many
//! projects they affect.
//!
//! Corpus entries are untrusted. Nothing acquired here is executed: the two
//! tool executables are resolved once against the invocation's own project
//! root and pinned as absolute paths, so an entry that ships its own
//! `node_modules/.bin` can never be run, and no project configuration is
//! evaluated.

pub mod acquire;
pub mod analysis;
pub mod archive;
pub mod cache;
pub mod entry;
pub mod finding;
pub mod group;
pub mod integrity;
pub mod manifest;
pub mod normalize;
pub mod reduction;
pub mod report;
pub mod state;

use std::collections::BTreeMap;
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use rayon::prelude::*;

use crate::capabilities::CapabilityCatalog;
use crate::config::{Config, LoadedConfig, ToolConfig};
use crate::error::{ConcordError, Result};
use crate::model::{Tool, path_from_report};
use crate::process::ProcessRunner;
use crate::reduce::ReduceMode;
use crate::report::{ComparisonProfile, json};

use analysis::{AnalysisContext, EntryAnalysis};
use cache::Cache;
use entry::{EntryError, EntryStatus};
use finding::{Finding, FindingContext};
use manifest::ManifestEntry;
use reduction::PipelineReducer;
use report::{CorpusReport, ReportConfiguration, ReportEntry, ReportEntryError};
use state::{EntryRecord, RunState};

#[derive(Debug, Clone)]
pub struct CorpusOptions {
    pub manifest: PathBuf,
    pub mode: ReduceMode,
    pub baseline: Tool,
    pub candidate: Tool,
    pub profile: ComparisonProfile,
    pub normalize_eol: bool,
    pub cache_dir: Option<PathBuf>,
    pub registry: String,
    pub refresh: bool,
    pub jobs: usize,
    pub timeout: u64,
    pub acquire_timeout: u64,
    pub max_files_per_entry: usize,
    pub reduce: bool,
    pub reduce_timeout: u64,
    pub reduce_top: Option<usize>,
    pub no_resume: bool,
    pub quiet: bool,
    pub json: Option<PathBuf>,
    pub output: Option<PathBuf>,
}

impl CorpusOptions {
    pub fn mode_name(&self) -> &'static str {
        match self.mode {
            ReduceMode::Lint => "lint",
            ReduceMode::Format => "format",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CorpusOutcome {
    /// The run completed and produced no finding.
    Clean,
    /// The run completed and produced at least one finding.
    Findings,
    /// The run completed, at least one entry failed, and there is no finding.
    PartialFailure,
}

/// An absolute path inside a project, from a report-relative path.
pub(crate) fn project_file(root: &Path, relative: &str) -> PathBuf {
    path_from_report(root, relative)
}

pub fn run(loaded: &LoadedConfig, options: &CorpusOptions) -> Result<CorpusOutcome> {
    validate(options)?;
    let manifest = manifest::load(&options.manifest)?;
    for warning in &manifest.warnings {
        eprintln!("warning: {warning}");
    }
    let cache = Cache::open(options.cache_dir.as_deref())?;
    let config = pinned_config(loaded, options)?;
    let capabilities = CapabilityCatalog::new(&config)?;

    let key = state::key(&manifest, options, &config);
    let state_path = cache.state_path(&key);
    if options.no_resume {
        RunState::discard(&state_path);
    }
    let mut run_state = (!options.no_resume)
        .then(|| RunState::load(&state_path, &key))
        .flatten()
        .unwrap_or_else(|| RunState::new(&key));

    let progress = Progress::new(options.quiet, manifest.entries.len());
    let acquired = acquire_all(&manifest.entries, &cache, options, &progress);
    // A run with nothing to analyze and nothing already recorded has failed as
    // a whole. A fully resumed run still has its recorded results to report,
    // even if re-acquiring every entry would fail now.
    let recorded = manifest.entries.iter().any(|entry| {
        run_state
            .entries
            .contains_key(&entry.identity.manifest_form())
    });
    if acquired.roots.is_empty() && !recorded {
        return Err(ConcordError::run_failure(
            "every corpus entry failed acquisition; no project could be analyzed",
        ));
    }

    let context = AnalysisContext {
        config: config.clone(),
        capabilities,
        finding: FindingContext {
            mode: options.mode,
            baseline: options.baseline,
            candidate: options.candidate,
        },
        profile: options.profile,
        max_files: options.max_files_per_entry,
        timeout: Duration::from_secs(options.timeout),
        normalize_eol: options.normalize_eol,
    };
    let pending: Vec<&ManifestEntry> = manifest
        .entries
        .iter()
        .filter(|entry| {
            !run_state
                .entries
                .contains_key(&entry.identity.manifest_form())
        })
        .collect();
    progress.resumed(manifest.entries.len() - pending.len());

    let shared = Mutex::new(SharedState {
        state: run_state,
        warned: false,
    });
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(options.jobs)
        .build()
        .map_err(|error| {
            ConcordError::run_failure(format!("failed to create the corpus worker pool: {error}"))
        })?;
    pool.install(|| {
        pending.par_iter().for_each(|entry| {
            let identity = entry.identity.manifest_form();
            let root = acquired.roots.get(&identity);
            let record = match root {
                Some(root) => match analysis::analyze(&context, &identity, root) {
                    Ok(analysis) => analyzed(analysis),
                    Err(error) => failed(error.redacted(Some(root))),
                },
                None => failed(
                    acquired
                        .failures
                        .get(&identity)
                        .cloned()
                        .unwrap_or_else(|| EntryError::acquisition("entry was not acquired"))
                        .redacted(None),
                ),
            };
            let findings = record.findings.len();
            if let Ok(mut guard) = shared.lock() {
                guard.state.entries.insert(identity.clone(), record);
                if let Err(error) = guard.state.save(&state_path) {
                    if !guard.warned {
                        guard.warned = true;
                        eprintln!("warning: run state could not be persisted: {error}");
                    }
                }
            }
            progress.completed(&identity, findings);
        });
    });
    run_state = match shared.into_inner() {
        Ok(shared) => shared.state,
        Err(poisoned) => poisoned.into_inner().state,
    };

    let (entries, entry_errors, findings) = collect(&manifest.entries, &run_state, options);
    let mut groups = group::group(findings);
    if options.reduce {
        let reducer = PipelineReducer::new(
            config.clone(),
            options.mode,
            options.baseline,
            options.candidate,
        );
        progress.reducing(groups.len(), options.reduce_top);
        reduction::reduce_groups(
            &mut groups,
            &reducer,
            &acquired.roots,
            options.reduce_top,
            Duration::from_secs(options.reduce_timeout),
            |group| progress.reduced(group),
        );
    }

    let report = CorpusReport::build(
        configuration(options, &config),
        entries,
        entry_errors,
        &groups,
        options.baseline,
        options.candidate,
    );
    if let Some(path) = &options.json {
        json::save_to(path, &report).map_err(|error| {
            ConcordError::run_failure(format!(
                "failed to write the corpus JSON artifact\npath: {}\nerror: {error}",
                path.display()
            ))
        })?;
    }
    write_report(options.output.as_deref(), &report::render(&report))?;

    Ok(if report.has_findings() {
        CorpusOutcome::Findings
    } else if report.has_failures() {
        CorpusOutcome::PartialFailure
    } else {
        CorpusOutcome::Clean
    })
}

struct SharedState {
    state: RunState,
    warned: bool,
}

#[derive(Default)]
struct Acquisition {
    roots: BTreeMap<String, PathBuf>,
    failures: BTreeMap<String, EntryError>,
}

/// Acquire every entry before any analysis begins, so that a network problem
/// surfaces immediately rather than halfway through a long run.
fn acquire_all(
    entries: &[ManifestEntry],
    cache: &Cache,
    options: &CorpusOptions,
    progress: &Progress,
) -> Acquisition {
    let acquire_options = acquire::AcquireOptions {
        registry: options.registry.clone(),
        timeout: Duration::from_secs(options.acquire_timeout),
        refresh: options.refresh,
    };
    let mut acquisition = Acquisition::default();
    for entry in entries {
        let identity = entry.identity.manifest_form();
        progress.acquiring(&identity);
        match acquire::acquire(&entry.identity, cache, &acquire_options) {
            Ok(acquired) => {
                acquisition.roots.insert(identity, acquired.root);
            }
            Err(error) => {
                acquisition.failures.insert(identity, error);
            }
        }
    }
    acquisition
}

fn collect(
    entries: &[ManifestEntry],
    run_state: &RunState,
    options: &CorpusOptions,
) -> (Vec<ReportEntry>, Vec<ReportEntryError>, Vec<Finding>) {
    let mut reported = Vec::new();
    let mut errors = Vec::new();
    let mut findings = Vec::new();
    for entry in entries {
        let identity = entry.identity.manifest_form();
        let Some(record) = run_state.entries.get(&identity) else {
            continue;
        };
        if let Some(error) = &record.error {
            errors.push(report::entry_error(&identity, error));
        }
        reported.push(ReportEntry {
            project: identity.clone(),
            scheme: entry.identity.scheme().to_owned(),
            status: record.status,
            files_discovered: record.files_discovered,
            files_analyzed: record.files_analyzed,
            truncated: record.truncated,
            max_files_per_entry: record.truncated.then_some(options.max_files_per_entry),
            occurrences: record.findings.len(),
        });
        findings.extend(record.findings.iter().cloned());
    }
    (reported, errors, findings)
}

fn analyzed(analysis: EntryAnalysis) -> EntryRecord {
    EntryRecord {
        status: EntryStatus::Analyzed,
        findings: analysis.findings,
        error: None,
        files_discovered: analysis.files_discovered,
        files_analyzed: analysis.files_analyzed,
        truncated: analysis.truncated,
    }
}

fn failed(error: EntryError) -> EntryRecord {
    EntryRecord {
        status: EntryStatus::Failed,
        findings: Vec::new(),
        error: Some(error),
        files_discovered: 0,
        files_analyzed: 0,
        truncated: false,
    }
}

fn configuration(options: &CorpusOptions, config: &Config) -> ReportConfiguration {
    ReportConfiguration {
        mode: options.mode_name().to_owned(),
        baseline_tool: options.baseline,
        candidate_tool: options.candidate,
        profile: options.profile,
        normalize_eol: options.normalize_eol,
        count_probable_as_match: config.matching.count_probable_as_match,
        max_files_per_entry: options.max_files_per_entry,
        entry_timeout_seconds: options.timeout,
        acquire_timeout_seconds: options.acquire_timeout,
        registry: options.registry.clone(),
        reduce: options.reduce,
        reduce_top: options.reduce.then_some(options.reduce_top).flatten(),
        reduce_timeout_seconds: options.reduce.then_some(options.reduce_timeout),
    }
}

fn validate(options: &CorpusOptions) -> Result<()> {
    if options.baseline == options.candidate {
        return Err(ConcordError::usage(
            "baseline and candidate must be different tools",
        ));
    }
    match options.mode {
        ReduceMode::Lint if !options.baseline.is_linter() || !options.candidate.is_linter() => {
            return Err(ConcordError::usage(
                "lint corpus runs support eslint, biome, and oxlint",
            ));
        }
        ReduceMode::Format
            if !options.baseline.is_formatter() || !options.candidate.is_formatter() =>
        {
            return Err(ConcordError::usage(
                "format corpus runs support prettier, biome, and oxfmt",
            ));
        }
        _ => {}
    }
    for (name, value) in [
        ("--jobs", options.jobs),
        ("--max-files-per-entry", options.max_files_per_entry),
    ] {
        if value == 0 {
            return Err(ConcordError::usage(format!(
                "{name} must be greater than zero"
            )));
        }
    }
    for (name, value) in [
        ("--timeout", options.timeout),
        ("--acquire-timeout", options.acquire_timeout),
        ("--reduce-timeout", options.reduce_timeout),
    ] {
        if value == 0 {
            return Err(ConcordError::usage(format!(
                "{name} must be greater than zero seconds"
            )));
        }
    }
    Ok(())
}

/// Resolve both tools once, against the invocation's own project root, and pin
/// them as absolute commands. Nothing inside a corpus entry is ever resolved.
fn pinned_config(loaded: &LoadedConfig, options: &CorpusOptions) -> Result<Config> {
    let runner = ProcessRunner::new(loaded.root.clone(), loaded.config.clone(), None);
    let mut config = loaded.config.clone();
    for tool in [options.baseline, options.candidate] {
        let resolved = runner.resolve(tool).map_err(|error| {
            ConcordError::run_failure(format!(
                "{tool} must be available to run a corpus comparison\n{error}"
            ))
        })?;
        tool_config_mut(&mut config, tool).command =
            Some(resolved.executable.to_string_lossy().into_owned());
    }
    Ok(config)
}

fn tool_config_mut(config: &mut Config, tool: Tool) -> &mut ToolConfig {
    match tool {
        Tool::Eslint => &mut config.tools.eslint,
        Tool::Biome => &mut config.tools.biome,
        Tool::Oxlint => &mut config.tools.oxlint,
        Tool::Prettier => &mut config.tools.prettier,
        Tool::Oxfmt => &mut config.tools.oxfmt,
    }
}

fn write_report(path: Option<&Path>, contents: &str) -> Result<()> {
    let Some(path) = path else {
        print!("{contents}");
        let _ = std::io::stdout().flush();
        return Ok(());
    };
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent).map_err(|error| {
            ConcordError::run_failure(format!(
                "failed to create the report directory\npath: {}\nerror: {error}",
                parent.display()
            ))
        })?;
    }
    std::fs::write(path, contents).map_err(|error| {
        ConcordError::run_failure(format!(
            "failed to write the corpus report\npath: {}\nerror: {error}",
            path.display()
        ))
    })
}

/// Progress goes to stderr and never to the report. It is suppressed when
/// stderr is not a terminal or when `--quiet` is passed.
struct Progress {
    enabled: bool,
    total: usize,
    counters: Mutex<Counters>,
}

#[derive(Default)]
struct Counters {
    completed: usize,
    findings: usize,
}

impl Progress {
    fn new(quiet: bool, total: usize) -> Self {
        Self {
            enabled: !quiet && std::io::stderr().is_terminal(),
            total,
            counters: Mutex::new(Counters::default()),
        }
    }

    fn acquiring(&self, identity: &str) {
        if self.enabled {
            eprintln!("acquiring {identity}");
        }
    }

    fn resumed(&self, count: usize) {
        if self.enabled && count > 0 {
            eprintln!("resuming: {count}/{} entries already recorded", self.total);
            if let Ok(mut counters) = self.counters.lock() {
                counters.completed = count;
            }
        }
    }

    fn completed(&self, identity: &str, findings: usize) {
        if !self.enabled {
            return;
        }
        if let Ok(mut counters) = self.counters.lock() {
            counters.completed += 1;
            counters.findings += findings;
            eprintln!(
                "[{}/{}] {identity} — {} finding(s) so far",
                counters.completed, self.total, counters.findings
            );
        }
    }

    fn reducing(&self, groups: usize, top: Option<usize>) {
        if self.enabled {
            eprintln!(
                "reducing {} of {groups} finding group(s)",
                top.map_or(groups, |top| top.min(groups))
            );
        }
    }

    fn reduced(&self, group: &group::FindingGroup) {
        if !self.enabled {
            return;
        }
        let status = match &group.reduction {
            Some(reduction) if reduction.completed => "reduced".to_owned(),
            Some(reduction) => format!(
                "not reduced ({})",
                reduction.reason.as_deref().unwrap_or("unknown reason")
            ),
            None => "not reduced".to_owned(),
        };
        eprintln!(
            "{} {status}",
            &group.fingerprint[..12.min(group.fingerprint.len())]
        );
    }
}
