use std::path::PathBuf;

use thiserror::Error;

pub type Result<T> = std::result::Result<T, ConcordError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    Usage,
    Operational,
}

#[derive(Debug, Error)]
#[error("{message}")]
pub struct ConcordError {
    pub kind: ErrorKind,
    pub message: String,
}

impl ConcordError {
    pub fn usage(message: impl Into<String>) -> Self {
        Self {
            kind: ErrorKind::Usage,
            message: message.into(),
        }
    }

    pub fn operational(message: impl Into<String>) -> Self {
        Self {
            kind: ErrorKind::Operational,
            message: message.into(),
        }
    }

    pub fn io(context: impl AsRef<str>, path: &std::path::Path, source: std::io::Error) -> Self {
        Self::operational(format!(
            "{}\npath: {}\nerror: {source}",
            context.as_ref(),
            path.display()
        ))
    }
}

#[derive(Debug, Error)]
pub enum ToolFailure {
    #[error(
        "{tool} was not found.\n\nChecked:\n{checked}\n\nInstall it in the target project or configure tools.{config_key}.command\nin concord.toml."
    )]
    NotFound {
        tool: String,
        config_key: String,
        checked: String,
    },

    #[error("{tool} timed out after {seconds}s\ncommand: {executable} {arguments}")]
    Timeout {
        tool: String,
        seconds: u64,
        executable: String,
        arguments: String,
    },

    #[error("failed to start {tool}\ncommand: {executable} {arguments}\nerror: {source}")]
    Spawn {
        tool: String,
        executable: String,
        arguments: String,
        source: std::io::Error,
    },

    #[error(
        "{tool} failed\ncommand: {executable} {arguments}\nexit code: {exit_code:?}\nstderr: {stderr}"
    )]
    Exit {
        tool: String,
        executable: String,
        arguments: String,
        exit_code: Option<i32>,
        stderr: String,
    },

    #[error(
        "failed to parse {tool} {format} output\ncommand: {executable} {arguments}\nexit code: {exit_code:?}\nstderr: {stderr}\nerror: {detail}"
    )]
    InvalidOutput {
        tool: String,
        format: String,
        executable: PathBuf,
        arguments: String,
        exit_code: Option<i32>,
        stderr: String,
        detail: String,
    },
}

impl From<ToolFailure> for ConcordError {
    fn from(value: ToolFailure) -> Self {
        Self::operational(value.to_string())
    }
}

impl From<Box<ToolFailure>> for ConcordError {
    fn from(value: Box<ToolFailure>) -> Self {
        Self::operational(value.to_string())
    }
}
