use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;

use crate::error::{ConcordError, Result};

pub fn render<T: Serialize>(report: &T) -> Result<String> {
    serde_json::to_string_pretty(report).map_err(|error| {
        ConcordError::operational(format!("failed to serialize JSON report: {error}"))
    })
}

pub fn save<T: Serialize>(root: &Path, mode: &str, report: &T) -> Result<PathBuf> {
    let directory = root.join(".concord").join("reports");
    fs::create_dir_all(&directory).map_err(|error| {
        ConcordError::io("failed to create report directory", &directory, error)
    })?;
    let epoch_nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    let filename = format!("{mode}-{epoch_nanos}-{}.json", std::process::id());
    let path = directory.join(filename);
    let contents = render(report)?;
    fs::write(&path, contents)
        .map_err(|error| ConcordError::io("failed to save JSON report", &path, error))?;
    Ok(path)
}
