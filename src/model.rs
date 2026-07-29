use std::fmt;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Tool {
    Eslint,
    Biome,
    Oxlint,
    Prettier,
    Oxfmt,
}

impl Tool {
    pub const ALL: [Self; 5] = [
        Self::Eslint,
        Self::Biome,
        Self::Oxlint,
        Self::Prettier,
        Self::Oxfmt,
    ];

    pub fn display_name(self) -> &'static str {
        match self {
            Self::Eslint => "ESLint",
            Self::Biome => "Biome",
            Self::Oxlint => "Oxlint",
            Self::Prettier => "Prettier",
            Self::Oxfmt => "Oxfmt",
        }
    }

    pub fn config_key(self) -> &'static str {
        match self {
            Self::Eslint => "eslint",
            Self::Biome => "biome",
            Self::Oxlint => "oxlint",
            Self::Prettier => "prettier",
            Self::Oxfmt => "oxfmt",
        }
    }

    pub fn is_linter(self) -> bool {
        matches!(self, Self::Eslint | Self::Biome | Self::Oxlint)
    }

    pub fn is_formatter(self) -> bool {
        matches!(self, Self::Prettier | Self::Biome | Self::Oxfmt)
    }
}

impl fmt::Display for Tool {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.display_name())
    }
}

impl std::str::FromStr for Tool {
    type Err = String;

    fn from_str(value: &str) -> std::result::Result<Self, Self::Err> {
        match value.to_ascii_lowercase().as_str() {
            "eslint" => Ok(Self::Eslint),
            "biome" => Ok(Self::Biome),
            "oxlint" => Ok(Self::Oxlint),
            "prettier" => Ok(Self::Prettier),
            "oxfmt" => Ok(Self::Oxfmt),
            _ => Err(format!("unknown tool `{value}`")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Info,
    Warning,
    Error,
}

impl Severity {
    pub fn from_value(value: &serde_json::Value) -> Self {
        if let Some(number) = value.as_u64() {
            return match number {
                0 => Self::Info,
                1 => Self::Warning,
                _ => Self::Error,
            };
        }
        match value
            .as_str()
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str()
        {
            "fatal" | "error" | "2" => Self::Error,
            "warn" | "warning" | "1" => Self::Warning,
            _ => Self::Info,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Span {
    pub start_line: u32,
    pub start_column: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_line: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_column: Option<u32>,
}

impl Span {
    pub fn same_start(&self, other: &Self) -> bool {
        self.start_line == other.start_line && self.start_column == other.start_column
    }

    pub fn overlaps_or_same_line(&self, other: &Self) -> bool {
        if self.start_line == other.start_line {
            return true;
        }
        let self_end = self.end_line.unwrap_or(self.start_line);
        let other_end = other.end_line.unwrap_or(other.start_line);
        self.start_line <= other_end && other.start_line <= self_end
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Fix {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub replacement: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Diagnostic {
    pub tool: Tool,
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub canonical_code: Option<String>,
    pub severity: Severity,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub span: Option<Span>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fix: Option<Fix>,
    pub fingerprint: String,
}

#[derive(Debug, Clone)]
pub struct DiagnosticData {
    pub code: Option<String>,
    pub canonical_code: Option<String>,
    pub severity: Severity,
    pub message: String,
    pub span: Option<Span>,
    pub fix: Option<Fix>,
}

impl Diagnostic {
    pub fn new(tool: Tool, path: impl Into<String>, data: DiagnosticData) -> Self {
        let mut result = Self {
            tool,
            path: path.into(),
            code: data.code,
            canonical_code: data.canonical_code,
            severity: data.severity,
            message: data.message,
            span: data.span,
            fix: data.fix,
            fingerprint: String::new(),
        };
        result.refresh_fingerprint();
        result
    }

    pub fn refresh_fingerprint(&mut self) {
        let material = format!(
            "{:?}\0{}\0{:?}\0{:?}\0{}\0{:?}",
            self.tool, self.path, self.canonical_code, self.severity, self.message, self.span
        );
        self.fingerprint = format!("{:x}", Sha256::digest(material.as_bytes()));
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolRun {
    pub tool: Tool,
    pub executable: String,
    pub version: String,
    pub arguments: Vec<String>,
    pub exit_code: Option<i32>,
    pub duration_ms: u128,
    pub diagnostics: Vec<Diagnostic>,
    pub warnings: Vec<String>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub stdout: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub stderr: String,
}

pub fn normalize_path(root: &Path, input: &Path) -> String {
    let absolute = if input.is_absolute() {
        input.to_path_buf()
    } else {
        root.join(input)
    };
    let relative = absolute.strip_prefix(root).unwrap_or(&absolute);
    let mut parts: Vec<String> = Vec::new();
    for component in relative.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if parts.last().is_some_and(|part| part != "..") {
                    parts.pop();
                } else {
                    parts.push("..".into());
                }
            }
            Component::Normal(part) => parts.push(part.to_string_lossy().into_owned()),
            Component::RootDir => {}
            Component::Prefix(prefix) => {
                parts.push(prefix.as_os_str().to_string_lossy().into_owned());
            }
        }
    }
    if parts.is_empty() {
        ".".into()
    } else {
        parts.join("/")
    }
}

pub fn path_from_report(root: &Path, report_path: &str) -> PathBuf {
    let normalized = report_path.replace('/', std::path::MAIN_SEPARATOR_STR);
    root.join(normalized)
}

#[cfg(test)]
mod tests {
    use super::normalize_path;
    use std::path::Path;

    #[test]
    fn path_is_relative_and_clean() {
        let root = Path::new("/project");
        assert_eq!(
            normalize_path(root, Path::new("/project/src/./nested/../app.ts")),
            "src/app.ts"
        );
    }
}
