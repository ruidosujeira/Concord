use std::fs;
use std::io::Write;
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
    save_to(&path, report)?;
    Ok(path)
}

pub fn save_to<T: Serialize>(path: &Path, report: &T) -> Result<()> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)
        .map_err(|error| ConcordError::io("failed to create report directory", parent, error))?;
    let contents = render(report)?;
    let mut temporary = tempfile::Builder::new()
        .prefix(".concord-report-")
        .suffix(".tmp")
        .tempfile_in(parent)
        .map_err(|error| ConcordError::io("failed to create temporary report", parent, error))?;
    temporary
        .write_all(contents.as_bytes())
        .and_then(|()| temporary.flush())
        .and_then(|()| temporary.as_file().sync_all())
        .map_err(|error| {
            ConcordError::io("failed to write temporary report", temporary.path(), error)
        })?;
    let temporary_path = temporary.path().to_path_buf();
    match temporary.persist(path) {
        Ok(_) => Ok(()),
        Err(error) => {
            #[cfg(windows)]
            if error.error.kind() == std::io::ErrorKind::AlreadyExists {
                let temporary = error.file;
                fs::remove_file(path).map_err(|source| {
                    ConcordError::io("failed to replace existing report", path, source)
                })?;
                return temporary.persist(path).map(|_| ()).map_err(|error| {
                    ConcordError::io("failed to atomically replace report", path, error.error)
                });
            }
            let _ = fs::remove_file(&temporary_path);
            Err(ConcordError::io(
                "failed to atomically save JSON report",
                path,
                error.error,
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::save_to;

    #[test]
    fn writes_atomically_and_replaces_existing_report() {
        let directory = tempdir().expect("tempdir");
        let path = directory.path().join("nested/report with spaces.json");
        save_to(&path, &serde_json::json!({"schemaVersion": 2})).expect("first report");
        save_to(
            &path,
            &serde_json::json!({"schemaVersion": 2, "second": true}),
        )
        .expect("replacement report");
        let value: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).expect("report")).expect("complete JSON");
        assert_eq!(value["second"], true);
        assert!(
            fs::read_dir(path.parent().expect("parent"))
                .expect("directory")
                .all(|entry| !entry
                    .expect("entry")
                    .file_name()
                    .to_string_lossy()
                    .ends_with(".tmp"))
        );
    }
}
