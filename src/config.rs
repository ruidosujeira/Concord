use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::capabilities::{CapabilityCatalog, UnsupportedPolicy};
use crate::error::{ConcordError, Result};
use crate::matching::MappingConfidence;
use crate::model::Tool;

pub const DEFAULT_CONFIG: &str = r#"# Concord configuration
version = 1

[discovery]
include = ["**/*.js", "**/*.jsx", "**/*.mjs", "**/*.cjs", "**/*.ts", "**/*.tsx", "**/*.mts", "**/*.cts", "**/*.json", "**/*.jsonc"]
exclude = ["**/generated/**"]

[execution]
timeout_seconds = 30
formatter_jobs = 4

# Executables are resolved from node_modules/.bin, then PATH.
# Uncomment a command to select an explicit executable.
# [tools.eslint]
# command = "/path/to/eslint"
# include = ["**/*.js", "**/*.ts"]
# exclude = ["**/generated/**"]
# unsupported = []
#
# [tools.biome]
# command = "/path/to/biome"
#
# [tools.oxlint]
# command = "/path/to/oxlint"
#
# [tools.prettier]
# command = "/path/to/prettier"
#
# [tools.oxfmt]
# command = "/path/to/oxfmt"

[matching]
count_probable_as_match = false

# [[matching.rules]]
# baseline_tool = "eslint"
# baseline = "@typescript-eslint/no-unused-vars"
# candidate_tool = "biome"
# candidate = "lint/correctness/noUnusedVariables"
# confidence = "exact"
# notes = "Equivalent core intent; options may still affect behavior."

# Deprecated compatibility syntax:
# [[matching.aliases]]
# eslint = "@typescript-eslint/no-unused-vars"
# biome = "lint/correctness/noUnusedVariables"

[comparison]
unsupported = "difference"
"#;

fn default_version() -> u32 {
    1
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub version: u32,
    pub discovery: DiscoveryConfig,
    pub execution: ExecutionConfig,
    pub tools: ToolsConfig,
    pub matching: MatchingConfig,
    pub comparison: ComparisonConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            version: default_version(),
            discovery: DiscoveryConfig::default(),
            execution: ExecutionConfig::default(),
            tools: ToolsConfig::default(),
            matching: MatchingConfig::default(),
            comparison: ComparisonConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DiscoveryConfig {
    pub include: Vec<String>,
    pub exclude: Vec<String>,
}

impl Default for DiscoveryConfig {
    fn default() -> Self {
        Self {
            include: [
                "js", "jsx", "mjs", "cjs", "ts", "tsx", "mts", "cts", "json", "jsonc",
            ]
            .into_iter()
            .map(|extension| format!("**/*.{extension}"))
            .collect(),
            exclude: vec!["**/generated/**".into()],
        }
    }
}

fn default_timeout() -> u64 {
    30
}

fn default_jobs() -> usize {
    4
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ExecutionConfig {
    pub timeout_seconds: u64,
    pub formatter_jobs: usize,
}

impl Default for ExecutionConfig {
    fn default() -> Self {
        Self {
            timeout_seconds: default_timeout(),
            formatter_jobs: default_jobs(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ToolsConfig {
    pub eslint: ToolConfig,
    pub biome: ToolConfig,
    pub oxlint: ToolConfig,
    pub prettier: ToolConfig,
    pub oxfmt: ToolConfig,
}

impl ToolsConfig {
    pub fn get(&self, tool: Tool) -> &ToolConfig {
        match tool {
            Tool::Eslint => &self.eslint,
            Tool::Biome => &self.biome,
            Tool::Oxlint => &self.oxlint,
            Tool::Prettier => &self.prettier,
            Tool::Oxfmt => &self.oxfmt,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ToolConfig {
    pub command: Option<String>,
    pub include: Vec<String>,
    pub exclude: Vec<String>,
    pub unsupported: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ComparisonConfig {
    pub unsupported: UnsupportedPolicy,
}

impl Default for ComparisonConfig {
    fn default() -> Self {
        Self {
            unsupported: UnsupportedPolicy::Difference,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MatchingConfig {
    pub count_probable_as_match: bool,
    pub aliases: Vec<AliasConfig>,
    pub rules: Vec<RuleMappingConfig>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AliasConfig {
    pub eslint: Option<String>,
    pub biome: Option<String>,
    pub oxlint: Option<String>,
}

impl AliasConfig {
    pub fn values(&self) -> impl Iterator<Item = &str> {
        [
            self.eslint.as_deref(),
            self.biome.as_deref(),
            self.oxlint.as_deref(),
        ]
        .into_iter()
        .flatten()
    }

    pub fn tool_values(&self) -> impl Iterator<Item = (Tool, &str)> {
        [
            (Tool::Eslint, self.eslint.as_deref()),
            (Tool::Biome, self.biome.as_deref()),
            (Tool::Oxlint, self.oxlint.as_deref()),
        ]
        .into_iter()
        .filter_map(|(tool, value)| value.map(|value| (tool, value)))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuleMappingConfig {
    pub baseline_tool: Tool,
    pub baseline: String,
    pub candidate_tool: Tool,
    pub candidate: String,
    pub confidence: MappingConfidence,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

#[derive(Debug, Clone)]
pub struct LoadedConfig {
    pub config: Config,
    pub capabilities: CapabilityCatalog,
    pub root: PathBuf,
    pub path: Option<PathBuf>,
    pub warnings: Vec<String>,
}

pub fn load(explicit: Option<&Path>, cwd: &Path) -> Result<LoadedConfig> {
    let path = match explicit {
        Some(path) => Some(if path.is_absolute() {
            path.to_path_buf()
        } else {
            cwd.join(path)
        }),
        None => cwd
            .ancestors()
            .map(|ancestor| ancestor.join("concord.toml"))
            .find(|candidate| candidate.is_file()),
    };
    let root = path
        .as_ref()
        .and_then(|path| path.parent())
        .unwrap_or(cwd)
        .to_path_buf();
    let config = if let Some(path) = &path {
        let source = fs::read_to_string(path)
            .map_err(|error| ConcordError::io("failed to read configuration", path, error))?;
        let parsed: Config = toml::from_str(&source).map_err(|error| {
            ConcordError::usage(format!(
                "invalid Concord configuration\npath: {}\nerror: {error}",
                path.display()
            ))
        })?;
        if parsed.version != 1 {
            return Err(ConcordError::usage(format!(
                "unsupported concord.toml version {}; expected version 1",
                parsed.version
            )));
        }
        if parsed.execution.timeout_seconds == 0 {
            return Err(ConcordError::usage(
                "execution.timeout_seconds must be greater than zero",
            ));
        }
        if parsed.execution.formatter_jobs == 0 {
            return Err(ConcordError::usage(
                "execution.formatter_jobs must be greater than zero",
            ));
        }
        parsed
    } else {
        Config::default()
    };
    validate_rule_mappings(&config)?;
    let capabilities = CapabilityCatalog::new(&config)?;
    let warnings = if config.matching.aliases.is_empty() {
        Vec::new()
    } else {
        vec!["matching.aliases is deprecated; use matching.rules".into()]
    };
    Ok(LoadedConfig {
        config,
        capabilities,
        root,
        path,
        warnings,
    })
}

fn validate_rule_mappings(config: &Config) -> Result<()> {
    use std::collections::{BTreeMap, BTreeSet};

    let mut pairs = BTreeSet::new();
    let mut endpoints: BTreeMap<(Tool, String, Tool), String> = BTreeMap::new();
    for mapping in &config.matching.rules {
        if mapping.baseline_tool == mapping.candidate_tool {
            return Err(ConcordError::usage(
                "matching.rules cannot map a tool to itself",
            ));
        }
        if !mapping.baseline_tool.is_linter() || !mapping.candidate_tool.is_linter() {
            return Err(ConcordError::usage(
                "matching.rules only supports eslint, biome, and oxlint",
            ));
        }
        let baseline = mapping.baseline.trim();
        let candidate = mapping.candidate.trim();
        if baseline.is_empty() || candidate.is_empty() {
            return Err(ConcordError::usage(
                "matching.rules rule codes must not be empty",
            ));
        }
        let mut pair = [
            (mapping.baseline_tool, baseline.to_owned()),
            (mapping.candidate_tool, candidate.to_owned()),
        ];
        pair.sort();
        if !pairs.insert(pair.clone()) {
            return Err(ConcordError::usage(format!(
                "duplicate matching.rules mapping: {}:{} <-> {}:{}",
                pair[0].0.config_key(),
                pair[0].1,
                pair[1].0.config_key(),
                pair[1].1
            )));
        }
        for ((tool, code), (other_tool, other_code)) in [(&pair[0], &pair[1]), (&pair[1], &pair[0])]
        {
            let key = (*tool, code.clone(), *other_tool);
            if let Some(existing) = endpoints.insert(key, other_code.clone()) {
                if existing != *other_code {
                    return Err(ConcordError::usage(format!(
                        "conflicting matching.rules mapping for {}:{}",
                        tool.config_key(),
                        code
                    )));
                }
            }
        }
    }
    Ok(())
}

pub fn init(path: &Path, force: bool) -> Result<()> {
    if path.exists() && !force {
        return Err(ConcordError::usage(format!(
            "{} already exists; use --force to overwrite it",
            path.display()
        )));
    }
    fs::write(path, DEFAULT_CONFIG)
        .map_err(|error| ConcordError::io("failed to write configuration", path, error))?;
    let _: Config = toml::from_str(DEFAULT_CONFIG).map_err(|error| {
        ConcordError::operational(format!("generated configuration is invalid: {error}"))
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::{Config, DEFAULT_CONFIG, load};

    #[test]
    fn default_config_is_valid() {
        let config: Config = toml::from_str(DEFAULT_CONFIG).expect("valid built-in config");
        assert_eq!(config.version, 1);
    }

    #[test]
    fn unknown_fields_are_rejected() {
        let error = toml::from_str::<Config>("version = 1\nmystery = true")
            .expect_err("unknown field should fail");
        assert!(error.to_string().contains("unknown field"));
    }

    #[test]
    fn version_one_configuration_keeps_new_defaults() {
        let config: Config = toml::from_str(
            "version = 1\n[tools.eslint]\ncommand = \"eslint\"\n[matching]\ncount_probable_as_match = true\n",
        )
        .expect("v0.1 configuration");
        assert!(config.tools.eslint.include.is_empty());
        assert!(config.tools.eslint.exclude.is_empty());
        assert!(config.tools.eslint.unsupported.is_empty());
        assert!(config.matching.rules.is_empty());
        assert_eq!(
            config.comparison.unsupported,
            crate::capabilities::UnsupportedPolicy::Difference
        );
    }

    #[test]
    fn real_v01_configuration_loads_without_changes() {
        let config: Config = toml::from_str(include_str!("../tests/fixtures/config-v0.1.toml"))
            .expect("v0.1 fixture");
        assert_eq!(config.version, 1);
        assert_eq!(config.tools.eslint.command.as_deref(), Some("eslint"));
        assert!(config.matching.rules.is_empty());
    }

    #[test]
    fn invalid_tool_glob_is_a_clear_configuration_error() {
        let directory = tempdir().expect("tempdir");
        fs::write(
            directory.path().join("concord.toml"),
            "version = 1\n[tools.biome]\nexclude = [\"[invalid\"]\n",
        )
        .expect("config");
        let error = load(None, directory.path()).expect_err("invalid glob");
        assert!(error.to_string().contains("tools.biome.exclude"));
        assert!(error.to_string().contains("[invalid"));
    }

    #[test]
    fn alias_deprecation_warning_is_stable() {
        let directory = tempdir().expect("tempdir");
        fs::write(
            directory.path().join("concord.toml"),
            "version = 1\n[[matching.aliases]]\neslint = \"no-debugger\"\nbiome = \"lint/suspicious/noDebugger\"\n",
        )
        .expect("config");
        let loaded = load(None, directory.path()).expect("loaded config");
        assert_eq!(
            loaded.warnings,
            ["matching.aliases is deprecated; use matching.rules"]
        );
    }

    #[test]
    fn duplicate_and_conflicting_rule_mappings_are_rejected() {
        let duplicate = r#"
version = 1
[[matching.rules]]
baseline_tool = "eslint"
baseline = "a"
candidate_tool = "biome"
candidate = "b"
confidence = "exact"
[[matching.rules]]
baseline_tool = "biome"
baseline = "b"
candidate_tool = "eslint"
candidate = "a"
confidence = "exact"
"#;
        let directory = tempdir().expect("tempdir");
        fs::write(directory.path().join("concord.toml"), duplicate).expect("config");
        assert!(
            load(None, directory.path())
                .expect_err("duplicate")
                .to_string()
                .contains("duplicate")
        );

        let conflict = duplicate.replace(
            "baseline = \"b\"\ncandidate_tool = \"eslint\"\ncandidate = \"a\"",
            "baseline = \"other\"\ncandidate_tool = \"eslint\"\ncandidate = \"a\"",
        );
        fs::write(directory.path().join("concord.toml"), conflict).expect("config");
        assert!(
            load(None, directory.path())
                .expect_err("conflict")
                .to_string()
                .contains("conflicting")
        );
    }
}
