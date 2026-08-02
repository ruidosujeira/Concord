use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use clap::ValueEnum;
use globset::{Glob, GlobSet, GlobSetBuilder};
use serde::{Deserialize, Serialize};

use crate::config::{Config, ToolConfig};
use crate::error::{ConcordError, Result};
use crate::model::{Tool, normalize_path};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CapabilityStatus {
    Supported,
    Unsupported,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CapabilitySource {
    BuiltIn,
    Configuration,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityDecision {
    pub status: CapabilityStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub source: CapabilitySource,
}

impl CapabilityDecision {
    fn supported(reason: impl Into<String>) -> Self {
        Self {
            status: CapabilityStatus::Supported,
            reason: Some(reason.into()),
            source: CapabilitySource::BuiltIn,
        }
    }

    fn unknown() -> Self {
        Self {
            status: CapabilityStatus::Unknown,
            reason: Some("support is not known; Concord will try the tool".into()),
            source: CapabilitySource::Unknown,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "lowercase")]
#[clap(rename_all = "lower")]
pub enum UnsupportedPolicy {
    Ignore,
    Difference,
    Error,
}

#[derive(Debug, Clone)]
struct ToolPatterns {
    include: GlobSet,
    has_include: bool,
    exclude: GlobSet,
    unsupported: GlobSet,
}

#[derive(Debug, Clone)]
pub struct CapabilityCatalog {
    patterns: BTreeMap<Tool, ToolPatterns>,
}

impl CapabilityCatalog {
    pub fn new(config: &Config) -> Result<Self> {
        let mut patterns = BTreeMap::new();
        for tool in Tool::ALL {
            patterns.insert(tool, compile_tool_patterns(tool, config.tools.get(tool))?);
        }
        Ok(Self { patterns })
    }

    pub fn classify(
        &self,
        root: &Path,
        tool: Tool,
        path: &Path,
        version: Option<&str>,
    ) -> PlannedToolFile {
        let normalized = normalize_path(root, path);
        let patterns = self
            .patterns
            .get(&tool)
            .expect("all tools have compiled capability patterns");
        if patterns.unsupported.is_match(&normalized) {
            return PlannedToolFile {
                tool,
                status: PlannedToolStatus::Unsupported,
                reason: Some("declared unsupported by configuration".into()),
                source: CapabilitySource::Configuration,
            };
        }
        if patterns.exclude.is_match(&normalized) {
            return PlannedToolFile {
                tool,
                status: PlannedToolStatus::Skipped,
                reason: Some("excluded by configuration".into()),
                source: CapabilitySource::Configuration,
            };
        }
        if patterns.has_include && !patterns.include.is_match(&normalized) {
            return PlannedToolFile {
                tool,
                status: PlannedToolStatus::Skipped,
                reason: Some("not included by configuration".into()),
                source: CapabilitySource::Configuration,
            };
        }
        let decision = capability_for_path(tool, path, version);
        PlannedToolFile {
            tool,
            status: match decision.status {
                CapabilityStatus::Supported => PlannedToolStatus::Supported,
                CapabilityStatus::Unsupported => PlannedToolStatus::Unsupported,
                CapabilityStatus::Unknown => PlannedToolStatus::Unknown,
            },
            reason: decision.reason,
            source: decision.source,
        }
    }
}

fn compile_tool_patterns(tool: Tool, config: &ToolConfig) -> Result<ToolPatterns> {
    Ok(ToolPatterns {
        include: compile_globs(
            &config.include,
            &format!("tools.{}.include", tool.config_key()),
        )?,
        has_include: !config.include.is_empty(),
        exclude: compile_globs(
            &config.exclude,
            &format!("tools.{}.exclude", tool.config_key()),
        )?,
        unsupported: compile_globs(
            &config.unsupported,
            &format!("tools.{}.unsupported", tool.config_key()),
        )?,
    })
}

fn compile_globs(patterns: &[String], field: &str) -> Result<GlobSet> {
    let mut builder = GlobSetBuilder::new();
    for pattern in patterns {
        builder.add(Glob::new(pattern).map_err(|error| {
            ConcordError::usage(format!(
                "invalid glob in {field}: `{pattern}`\nerror: {error}"
            ))
        })?);
    }
    builder.build().map_err(|error| {
        ConcordError::usage(format!("failed to compile globs in {field}: {error}"))
    })
}

pub fn capability_for_path(tool: Tool, path: &Path, version: Option<&str>) -> CapabilityDecision {
    if tool == Tool::Oxfmt
        && path
            .file_name()
            .is_some_and(|name| name == "package-lock.json")
    {
        if exact_version(version) == Some("0.60.0") {
            return CapabilityDecision {
                status: CapabilityStatus::Unsupported,
                reason: Some("Oxfmt 0.60.0 does not support package-lock.json via stdin".into()),
                source: CapabilitySource::BuiltIn,
            };
        }
        return CapabilityDecision::unknown();
    }
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let supported = match tool {
        Tool::Eslint | Tool::Oxlint => matches!(
            extension.as_str(),
            "js" | "jsx" | "mjs" | "cjs" | "ts" | "tsx" | "mts" | "cts"
        ),
        Tool::Biome => matches!(
            extension.as_str(),
            "js" | "jsx" | "mjs" | "cjs" | "ts" | "tsx" | "mts" | "cts" | "json" | "jsonc"
        ),
        Tool::Prettier | Tool::Oxfmt => matches!(
            extension.as_str(),
            "js" | "jsx" | "mjs" | "cjs" | "ts" | "tsx" | "mts" | "cts" | "json" | "jsonc"
        ),
    };
    if supported {
        CapabilityDecision::supported("file extension is supported by the built-in adapter")
    } else {
        CapabilityDecision::unknown()
    }
}

fn exact_version(version: Option<&str>) -> Option<&str> {
    version?
        .split(|character: char| character.is_whitespace() || character == 'v')
        .find(|part| *part == "0.60.0")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PlannedToolStatus {
    Supported,
    Unsupported,
    Unknown,
    Skipped,
}

impl PlannedToolStatus {
    pub fn is_comparable(self) -> bool {
        matches!(self, Self::Supported | Self::Unknown)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlannedToolFile {
    pub tool: Tool,
    pub status: PlannedToolStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub source: CapabilitySource,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlannedFile {
    pub path: String,
    pub baseline: PlannedToolFile,
    pub candidate: PlannedToolFile,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileDisposition {
    pub path: String,
    pub side: ComparisonSide,
    pub tool: Tool,
    pub reason: String,
    pub source: CapabilitySource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ComparisonSide {
    Baseline,
    Candidate,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolAvailability {
    pub tool: Tool,
    pub available: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub executable: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ComparisonPlan {
    pub discovered: Vec<PlannedFile>,
    pub comparable: Vec<String>,
    pub unsupported: Vec<FileDisposition>,
    pub skipped: Vec<FileDisposition>,
    pub tools: Vec<ToolAvailability>,
}

impl ComparisonPlan {
    #[allow(clippy::too_many_arguments)]
    pub fn build(
        root: &Path,
        files: &[PathBuf],
        baseline: Tool,
        candidate: Tool,
        baseline_version: Option<&str>,
        candidate_version: Option<&str>,
        catalog: &CapabilityCatalog,
        tools: Vec<ToolAvailability>,
    ) -> Self {
        let mut discovered = Vec::new();
        let mut comparable = Vec::new();
        let mut unsupported = Vec::new();
        let mut skipped = Vec::new();
        for path in files {
            let normalized = normalize_path(root, path);
            let baseline_plan = catalog.classify(root, baseline, path, baseline_version);
            let candidate_plan = catalog.classify(root, candidate, path, candidate_version);
            if baseline_plan.status.is_comparable() && candidate_plan.status.is_comparable() {
                comparable.push(normalized.clone());
            }
            collect_disposition(
                &normalized,
                ComparisonSide::Baseline,
                &baseline_plan,
                &mut unsupported,
                &mut skipped,
            );
            collect_disposition(
                &normalized,
                ComparisonSide::Candidate,
                &candidate_plan,
                &mut unsupported,
                &mut skipped,
            );
            discovered.push(PlannedFile {
                path: normalized,
                baseline: baseline_plan,
                candidate: candidate_plan,
            });
        }
        discovered.sort_by(|left, right| left.path.cmp(&right.path));
        comparable.sort();
        unsupported.sort_by(disposition_order);
        skipped.sort_by(disposition_order);
        let mut tools = tools;
        tools.sort_by_key(|tool| tool.tool);
        Self {
            discovered,
            comparable,
            unsupported,
            skipped,
            tools,
        }
    }

    pub fn discovered_count(&self) -> usize {
        self.discovered.len()
    }

    pub fn comparable_count(&self) -> usize {
        self.comparable.len()
    }

    pub fn unsupported_count(&self) -> usize {
        unique_paths(&self.unsupported)
    }

    pub fn skipped_count(&self) -> usize {
        let unsupported: BTreeSet<_> = self.unsupported.iter().map(|item| &item.path).collect();
        self.skipped
            .iter()
            .filter(|item| !unsupported.contains(&item.path))
            .map(|item| &item.path)
            .collect::<BTreeSet<_>>()
            .len()
    }

    pub fn comparable_paths(&self, root: &Path) -> Vec<PathBuf> {
        self.comparable.iter().map(|path| root.join(path)).collect()
    }
}

fn unique_paths(values: &[FileDisposition]) -> usize {
    values
        .iter()
        .map(|item| &item.path)
        .collect::<BTreeSet<_>>()
        .len()
}

fn disposition_order(left: &FileDisposition, right: &FileDisposition) -> std::cmp::Ordering {
    left.path
        .cmp(&right.path)
        .then_with(|| left.side.cmp(&right.side))
        .then_with(|| left.tool.cmp(&right.tool))
}

impl PartialOrd for ComparisonSide {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for ComparisonSide {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        (*self as u8).cmp(&(*other as u8))
    }
}

fn collect_disposition(
    path: &str,
    side: ComparisonSide,
    planned: &PlannedToolFile,
    unsupported: &mut Vec<FileDisposition>,
    skipped: &mut Vec<FileDisposition>,
) {
    let destination = match planned.status {
        PlannedToolStatus::Unsupported => unsupported,
        PlannedToolStatus::Skipped => skipped,
        _ => return,
    };
    destination.push(FileDisposition {
        path: path.into(),
        side,
        tool: planned.tool,
        reason: planned
            .reason
            .clone()
            .unwrap_or_else(|| "unspecified".into()),
        source: planned.source,
    });
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{
        CapabilityCatalog, CapabilitySource, CapabilityStatus, PlannedToolStatus,
        capability_for_path,
    };
    use crate::config::Config;
    use crate::model::Tool;

    #[test]
    fn known_extension_is_supported() {
        let decision = capability_for_path(Tool::Prettier, Path::new("app.ts"), None);
        assert_eq!(decision.status, CapabilityStatus::Supported);
    }

    #[test]
    fn unknown_extension_is_attempted() {
        let decision = capability_for_path(Tool::Prettier, Path::new("template.foo"), None);
        assert_eq!(decision.status, CapabilityStatus::Unknown);
        assert_eq!(decision.source, CapabilitySource::Unknown);
    }

    #[test]
    fn oxfmt_capability_is_versioned() {
        let known = capability_for_path(
            Tool::Oxfmt,
            Path::new("package-lock.json"),
            Some("oxfmt 0.60.0"),
        );
        assert_eq!(known.status, CapabilityStatus::Unsupported);
        let future =
            capability_for_path(Tool::Oxfmt, Path::new("package-lock.json"), Some("0.61.0"));
        assert_eq!(future.status, CapabilityStatus::Unknown);
    }

    #[test]
    fn configuration_distinguishes_unsupported_and_skipped() {
        let mut config = Config::default();
        config.tools.biome.unsupported = vec!["**/lock.json".into()];
        config.tools.biome.exclude = vec!["**/generated/**".into()];
        config.tools.biome.include = vec!["**/*.json".into()];
        let catalog = CapabilityCatalog::new(&config).expect("catalog");
        assert_eq!(
            catalog
                .classify(
                    Path::new("/project"),
                    Tool::Biome,
                    Path::new("/project/lock.json"),
                    None,
                )
                .status,
            PlannedToolStatus::Unsupported
        );
        assert_eq!(
            catalog
                .classify(
                    Path::new("/project"),
                    Tool::Biome,
                    Path::new("/project/generated/file.json"),
                    None,
                )
                .status,
            PlannedToolStatus::Skipped
        );
        assert_eq!(
            catalog
                .classify(
                    Path::new("/project"),
                    Tool::Biome,
                    Path::new("/project/file.ts"),
                    None,
                )
                .status,
            PlannedToolStatus::Skipped
        );
    }

    #[test]
    fn comparison_plan_paths_are_sorted() {
        let config = Config::default();
        let catalog = CapabilityCatalog::new(&config).expect("catalog");
        let plan = super::ComparisonPlan::build(
            Path::new("/project"),
            &["/project/z.ts".into(), "/project/a.ts".into()],
            Tool::Eslint,
            Tool::Biome,
            None,
            None,
            &catalog,
            Vec::new(),
        );
        assert_eq!(plan.comparable, ["a.ts", "z.ts"]);
        assert_eq!(plan.discovered[0].path, "a.ts");
    }
}
