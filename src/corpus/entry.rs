//! Per-entry status and failure reporting.
//!
//! Every failure of a single entry is caught, recorded against that entry with
//! a machine-readable kind, and never aborts the run.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryErrorKind {
    /// The entry could not be downloaded, verified or unpacked.
    Acquisition,
    /// The project root could not be read.
    Unreadable,
    /// Nothing in the project matches the configured discovery patterns.
    NoAnalyzableFiles,
    /// A linter or formatter failed, crashed or emitted unusable output.
    ToolFailure,
    /// A source file is not valid UTF-8.
    NotUtf8,
    /// The entry exceeded its analysis budget and was abandoned.
    Timeout,
    /// A worker panicked while analyzing the entry.
    Panic,
}

impl EntryErrorKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Acquisition => "acquisition",
            Self::Unreadable => "unreadable",
            Self::NoAnalyzableFiles => "no_analyzable_files",
            Self::ToolFailure => "tool_failure",
            Self::NotUtf8 => "not_utf8",
            Self::Timeout => "timeout",
            Self::Panic => "panic",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EntryError {
    pub kind: EntryErrorKind,
    pub message: String,
}

impl EntryError {
    pub fn new(kind: EntryErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    pub fn acquisition(message: impl Into<String>) -> Self {
        Self::new(EntryErrorKind::Acquisition, message)
    }

    /// Remove machine-specific locations from the recorded message. The
    /// project root becomes `<project>` and any other absolute path becomes
    /// `<path>`, so the artifact stays deterministic across machines.
    pub fn redacted(mut self, project_root: Option<&std::path::Path>) -> Self {
        if let Some(root) = project_root {
            let root = root.to_string_lossy();
            if !root.is_empty() {
                self.message = self.message.replace(root.as_ref(), "<project>");
            }
        }
        self.message = redact_absolute_paths(&self.message);
        self
    }
}

fn redact_absolute_paths(message: &str) -> String {
    let mut result = String::with_capacity(message.len());
    for token in split_keeping_whitespace(message) {
        if is_absolute_token(token) {
            result.push_str("<path>");
        } else {
            result.push_str(token);
        }
    }
    result
}

/// Split into alternating runs of whitespace and non-whitespace, so that the
/// original layout of a multi-line message survives redaction.
fn split_keeping_whitespace(value: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0;
    let mut whitespace = value.chars().next().is_some_and(char::is_whitespace);
    for (index, character) in value.char_indices() {
        if character.is_whitespace() != whitespace {
            parts.push(&value[start..index]);
            start = index;
            whitespace = !whitespace;
        }
    }
    if start < value.len() {
        parts.push(&value[start..]);
    }
    parts
}

fn is_absolute_token(token: &str) -> bool {
    let trimmed = token.trim_end_matches([',', ';', ':', ')', '"', '\'']);
    if trimmed.len() < 2 {
        return false;
    }
    if trimmed.starts_with('/') || trimmed.starts_with("\\\\") {
        return true;
    }
    let mut characters = trimmed.chars();
    matches!(characters.next(), Some(drive) if drive.is_ascii_alphabetic())
        && characters.next() == Some(':')
        && matches!(characters.next(), Some('\\' | '/'))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryStatus {
    Analyzed,
    Failed,
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{EntryError, EntryErrorKind};

    #[test]
    fn the_project_root_and_other_absolute_paths_are_redacted() {
        let error = EntryError::new(
            EntryErrorKind::ToolFailure,
            "ESLint timed out after 1s\ncommand: /opt/tools/bin/eslint --format json a.ts\n\
             path: /home/user/corpus/alpha/src/app.ts",
        )
        .redacted(Some(Path::new("/home/user/corpus/alpha")));
        assert_eq!(
            error.message,
            "ESLint timed out after 1s\ncommand: <path> --format json a.ts\npath: <project>/src/app.ts"
        );
    }

    #[test]
    fn windows_paths_and_unc_paths_are_redacted() {
        let error = EntryError::acquisition("failed at C:\\Users\\x\\cache and \\\\server\\share")
            .redacted(None);
        assert_eq!(error.message, "failed at <path> and <path>");
    }

    #[test]
    fn relative_paths_and_prose_survive_redaction() {
        let message = "no file matches the configured discovery patterns (src/app.ts, a:b)";
        let error = EntryError::acquisition(message).redacted(Some(Path::new("")));
        assert_eq!(error.message, message);
    }
}
