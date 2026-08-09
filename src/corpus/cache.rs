//! On-disk cache for acquired corpus entries and incremental run state.
//!
//! The layout is content-addressed by entry identity, so a cached entry is
//! reused without any network access. `path:` entries are never copied here.

use std::fs;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::error::{ConcordError, Result};

use super::manifest::EntryIdentity;

/// Written last, after a complete acquisition. An entry directory without it
/// is treated as absent, so an interrupted acquisition is never reused.
const MARKER: &str = ".concord-complete";

#[derive(Debug, Clone)]
pub struct Cache {
    root: PathBuf,
}

impl Cache {
    /// Open the cache root, creating it when necessary. An unusable cache
    /// directory fails the run.
    pub fn open(explicit: Option<&Path>) -> Result<Self> {
        let root = match explicit {
            Some(path) => path.to_path_buf(),
            None => default_root().ok_or_else(|| {
                ConcordError::run_failure(
                    "could not determine a user cache directory; pass --cache-dir",
                )
            })?,
        };
        fs::create_dir_all(&root).map_err(|error| {
            ConcordError::run_failure(format!(
                "unusable cache directory\npath: {}\nerror: {error}",
                root.display()
            ))
        })?;
        if !root.is_dir() {
            return Err(ConcordError::run_failure(format!(
                "unusable cache directory\npath: {}\nerror: not a directory",
                root.display()
            )));
        }
        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Where an acquired entry lives. `None` for local entries, which are read
    /// in place.
    pub fn entry_directory(&self, identity: &EntryIdentity) -> Option<PathBuf> {
        match identity {
            EntryIdentity::Npm { package, version } => {
                Some(self.root.join("npm").join(package).join(version))
            }
            EntryIdentity::Git { url, commit } => {
                Some(self.root.join("git").join(slug(url)).join(commit))
            }
            EntryIdentity::Path { .. } => None,
        }
    }

    pub fn state_path(&self, key: &str) -> PathBuf {
        self.root.join("runs").join(format!("{key}.json"))
    }
}

pub fn is_complete(directory: &Path) -> bool {
    directory.join(MARKER).is_file()
}

pub fn mark_complete(directory: &Path) -> std::io::Result<()> {
    fs::write(directory.join(MARKER), b"")
}

pub fn discard(directory: &Path) -> std::io::Result<()> {
    match fs::remove_dir_all(directory) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

/// A readable directory name for a repository URL, disambiguated by a digest
/// so that two URLs never collide after sanitization.
fn slug(url: &str) -> String {
    let readable: String = url
        .trim_start_matches("https://")
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.') {
                character
            } else {
                '-'
            }
        })
        .take(64)
        .collect();
    let digest = format!("{:x}", Sha256::digest(url.as_bytes()));
    format!("{}-{}", readable.trim_matches('-'), &digest[..16])
}

/// The platform user cache directory with a `concord` subdirectory.
pub fn default_root() -> Option<PathBuf> {
    use std::env;

    if cfg!(windows) {
        return env::var_os("LOCALAPPDATA")
            .filter(|value| !value.is_empty())
            .map(|value| PathBuf::from(value).join("concord").join("cache"));
    }
    let home = env::var_os("HOME").filter(|value| !value.is_empty());
    if cfg!(target_os = "macos") {
        return home.map(|value| {
            PathBuf::from(value)
                .join("Library")
                .join("Caches")
                .join("concord")
        });
    }
    env::var_os("XDG_CACHE_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| home.map(|value| PathBuf::from(value).join(".cache")))
        .map(|value| value.join("concord"))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use tempfile::tempdir;

    use super::{Cache, discard, is_complete, mark_complete, slug};
    use crate::corpus::manifest::EntryIdentity;

    #[test]
    fn entries_are_addressed_by_identity() {
        let directory = tempdir().expect("tempdir");
        let cache = Cache::open(Some(directory.path())).expect("cache");
        assert_eq!(
            cache.entry_directory(&EntryIdentity::Npm {
                package: "@scope/name".into(),
                version: "1.2.3".into()
            }),
            Some(
                directory
                    .path()
                    .join("npm")
                    .join("@scope/name")
                    .join("1.2.3")
            )
        );
        assert_eq!(
            cache.entry_directory(&EntryIdentity::Path {
                spec: "./local".into(),
                resolved: PathBuf::from("/local")
            }),
            None,
            "local entries are never cached"
        );
        let git = cache
            .entry_directory(&EntryIdentity::Git {
                url: "https://github.com/acme/repo".into(),
                commit: "a".repeat(40),
            })
            .expect("git directory");
        assert!(git.ends_with(PathBuf::from("a".repeat(40))));
        assert!(git.to_string_lossy().contains("github.com-acme-repo"));
    }

    #[test]
    fn similar_urls_do_not_collide() {
        assert_ne!(
            slug("https://github.com/acme/repo"),
            slug("https://github.com/acme/repo.git")
        );
        assert_ne!(
            slug("https://github.com/acme/repo"),
            slug("https://gitlab.com/acme/repo")
        );
    }

    #[test]
    fn completion_marker_round_trips_and_discards() {
        let directory = tempdir().expect("tempdir");
        let entry = directory.path().join("entry");
        std::fs::create_dir_all(&entry).expect("entry directory");
        assert!(!is_complete(&entry), "a partial extraction is not reused");
        mark_complete(&entry).expect("marker");
        assert!(is_complete(&entry));
        discard(&entry).expect("discard");
        assert!(!entry.exists());
        discard(&entry).expect("discarding a missing directory is not an error");
    }

    #[test]
    fn an_unusable_cache_directory_fails_the_run() {
        let directory = tempdir().expect("tempdir");
        let file = directory.path().join("file");
        std::fs::write(&file, b"").expect("file");
        let error = Cache::open(Some(&file.join("nested"))).expect_err("unusable cache");
        assert_eq!(error.kind, crate::error::ErrorKind::RunFailure);
        assert!(error.to_string().contains("unusable cache directory"));
    }
}
