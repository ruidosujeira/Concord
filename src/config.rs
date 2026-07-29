use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{ConcordError, Result};
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

# [[matching.aliases]]
# eslint = "@typescript-eslint/no-unused-vars"
# biome = "lint/correctness/noUnusedVariables"
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
}

impl Default for Config {
    fn default() -> Self {
        Self {
            version: default_version(),
            discovery: DiscoveryConfig::default(),
            execution: ExecutionConfig::default(),
            tools: ToolsConfig::default(),
            matching: MatchingConfig::default(),
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
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MatchingConfig {
    pub count_probable_as_match: bool,
    pub aliases: Vec<AliasConfig>,
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
}

#[derive(Debug, Clone)]
pub struct LoadedConfig {
    pub config: Config,
    pub root: PathBuf,
    pub path: Option<PathBuf>,
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
    Ok(LoadedConfig { config, root, path })
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
    use super::{Config, DEFAULT_CONFIG};

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
}
