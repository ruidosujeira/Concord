//! Incremental run state, enabling resume after interruption.
//!
//! State is keyed by a digest of the resolved manifest and of every
//! configuration value that affects results, so changing the corpus or such a
//! flag starts a fresh run instead of silently resuming an incompatible one.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::config::Config;
use crate::error::Result;
use crate::report::json;

use super::entry::{EntryError, EntryStatus};
use super::finding::Finding;
use super::manifest::Manifest;

pub const STATE_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EntryRecord {
    pub status: EntryStatus,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub findings: Vec<Finding>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<EntryError>,
    pub files_discovered: usize,
    pub files_analyzed: usize,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunState {
    pub state_version: u32,
    pub key: String,
    /// Keyed by project identity in manifest form.
    pub entries: BTreeMap<String, EntryRecord>,
}

impl RunState {
    pub fn new(key: impl Into<String>) -> Self {
        Self {
            state_version: STATE_VERSION,
            key: key.into(),
            entries: BTreeMap::new(),
        }
    }

    /// Load prior state for `key`. A missing, truncated, unreadable or
    /// incompatible state file is treated as absent rather than as an error.
    pub fn load(path: &Path, key: &str) -> Option<Self> {
        let contents = fs::read(path).ok()?;
        let state: Self = serde_json::from_slice(&contents).ok()?;
        (state.state_version == STATE_VERSION && state.key == key).then_some(state)
    }

    /// Write state to a temporary file and rename it into place, so that an
    /// interruption cannot leave a truncated state file behind.
    pub fn save(&self, path: &Path) -> Result<()> {
        json::save_to(path, self)
    }

    pub fn discard(path: &Path) {
        let _ = fs::remove_file(path);
    }
}

/// The resume key: the resolved manifest plus every result-affecting value.
/// Deliberately excluded are `--jobs`, `--cache-dir`, `--quiet`, the output
/// destinations and the reduction flags, none of which change which findings a
/// run produces.
pub fn key(manifest: &Manifest, options: &super::CorpusOptions, config: &Config) -> String {
    let components = [
        format!("concord-corpus-state/{STATE_VERSION}"),
        env!("CARGO_PKG_VERSION").to_owned(),
        manifest.digest(),
        options.mode_name().to_owned(),
        options.baseline.config_key().to_owned(),
        options.candidate.config_key().to_owned(),
        format!("{:?}", options.profile),
        options.max_files_per_entry.to_string(),
        options.normalize_eol.to_string(),
        options.timeout.to_string(),
        options.registry.clone(),
        serde_json::to_string(config).unwrap_or_default(),
    ];
    format!("{:x}", Sha256::digest(components.join("\u{1e}").as_bytes()))
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use tempfile::tempdir;

    use super::{EntryRecord, RunState, key};
    use crate::config::Config;
    use crate::corpus::CorpusOptions;
    use crate::corpus::entry::{EntryError, EntryErrorKind, EntryStatus};
    use crate::corpus::finding::tests::lint_finding;
    use crate::corpus::manifest::parse;
    use crate::model::Tool;
    use crate::reduce::ReduceMode;
    use crate::report::ComparisonProfile;

    fn options() -> CorpusOptions {
        CorpusOptions {
            manifest: PathBuf::from("corpus.txt"),
            mode: ReduceMode::Lint,
            baseline: Tool::Eslint,
            candidate: Tool::Biome,
            profile: ComparisonProfile::Raw,
            normalize_eol: false,
            cache_dir: None,
            registry: "https://registry.npmjs.org".into(),
            refresh: false,
            jobs: 4,
            timeout: 300,
            acquire_timeout: 120,
            max_files_per_entry: 2_000,
            reduce: false,
            reduce_timeout: 120,
            reduce_top: None,
            no_resume: false,
            quiet: false,
            json: None,
            output: None,
        }
    }

    fn state() -> RunState {
        let mut state = RunState::new("abc");
        state.entries.insert(
            "path:one".into(),
            EntryRecord {
                status: EntryStatus::Analyzed,
                findings: vec![lint_finding(
                    "baseline_only",
                    Some("no-debugger"),
                    "m",
                    "path:one",
                    "a.ts",
                    12,
                )],
                error: None,
                files_discovered: 3,
                files_analyzed: 3,
                truncated: false,
            },
        );
        state.entries.insert(
            "path:two".into(),
            EntryRecord {
                status: EntryStatus::Failed,
                findings: Vec::new(),
                error: Some(EntryError::new(EntryErrorKind::Timeout, "abandoned")),
                files_discovered: 0,
                files_analyzed: 0,
                truncated: false,
            },
        );
        state
    }

    #[test]
    fn state_round_trips_through_serialization() {
        let directory = tempdir().expect("tempdir");
        let path = directory.path().join("runs/state.json");
        let original = state();
        original.save(&path).expect("save state");
        let loaded = RunState::load(&path, "abc").expect("load state");
        assert_eq!(loaded, original);
    }

    #[test]
    fn a_state_file_for_another_key_is_not_reused() {
        let directory = tempdir().expect("tempdir");
        let path = directory.path().join("state.json");
        state().save(&path).expect("save state");
        assert!(RunState::load(&path, "a-different-key").is_none());
    }

    #[test]
    fn a_truncated_state_file_is_treated_as_absent() {
        let directory = tempdir().expect("tempdir");
        let path = directory.path().join("state.json");
        state().save(&path).expect("save state");
        let contents = fs::read_to_string(&path).expect("state contents");
        fs::write(&path, &contents[..contents.len() / 2]).expect("truncate");
        assert!(RunState::load(&path, "abc").is_none());
        fs::write(&path, b"").expect("empty");
        assert!(RunState::load(&path, "abc").is_none());
        assert!(RunState::load(&directory.path().join("missing.json"), "abc").is_none());
    }

    #[test]
    fn the_key_changes_with_the_manifest_and_with_result_affecting_flags() {
        let directory = std::path::Path::new("/manifests");
        let manifest = parse("path:one\n", directory).expect("manifest");
        let other = parse("path:one\npath:two\n", directory).expect("manifest");
        let config = Config::default();
        let base = key(&manifest, &options(), &config);

        assert_eq!(base, key(&manifest, &options(), &config), "stable");
        assert_ne!(base, key(&other, &options(), &config), "manifest");

        for mutate in [
            (|options: &mut CorpusOptions| options.mode = ReduceMode::Format)
                as fn(&mut CorpusOptions),
            |options| options.baseline = Tool::Oxlint,
            |options| options.profile = ComparisonProfile::Comparable,
            |options| options.max_files_per_entry = 10,
            |options| options.normalize_eol = true,
            |options| options.timeout = 30,
            |options| options.registry = "https://example.invalid".into(),
        ] {
            let mut changed = options();
            mutate(&mut changed);
            assert_ne!(base, key(&manifest, &changed, &config));
        }

        let mut changed_config = Config::default();
        changed_config.discovery.include = vec!["**/*.ts".into()];
        assert_ne!(base, key(&manifest, &options(), &changed_config));
    }

    #[test]
    fn the_key_ignores_flags_that_cannot_change_results() {
        let manifest = parse("path:one\n", std::path::Path::new("/m")).expect("manifest");
        let config = Config::default();
        let base = key(&manifest, &options(), &config);
        for mutate in [
            (|options: &mut CorpusOptions| options.jobs = 8) as fn(&mut CorpusOptions),
            |options| options.quiet = true,
            |options| options.reduce = true,
            |options| options.reduce_top = Some(1),
            |options| options.cache_dir = Some(PathBuf::from("/elsewhere")),
            |options| options.json = Some(PathBuf::from("out.json")),
        ] {
            let mut changed = options();
            mutate(&mut changed);
            assert_eq!(base, key(&manifest, &changed, &config));
        }
    }
}
