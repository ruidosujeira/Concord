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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryStatus {
    Analyzed,
    Failed,
}
