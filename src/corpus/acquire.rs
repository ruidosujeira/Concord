//! Acquisition of corpus entries from npm, git and the local filesystem.
//!
//! Nothing acquired here is ever executed: no install script runs, no package
//! manager is invoked, and no project configuration is evaluated. Entries are
//! unpacked as data and analyzed as data.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use command_group::CommandGroup;

use super::archive;
use super::cache::{self, Cache};
use super::entry::EntryError;
use super::integrity;
use super::manifest::EntryIdentity;

pub const DEFAULT_REGISTRY: &str = "https://registry.npmjs.org";

/// Transport failures are retried this many times; integrity failures and
/// missing packages are not.
const RETRIES: u32 = 2;
const METADATA_LIMIT: u64 = 8 * 1024 * 1024;
const TARBALL_LIMIT: u64 = 512 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct AcquireOptions {
    pub registry: String,
    pub timeout: Duration,
    pub refresh: bool,
}

#[derive(Debug, Clone)]
pub struct Acquired {
    pub root: PathBuf,
    pub from_cache: bool,
}

/// Make an entry available on disk. A cached entry is reused without any
/// network access.
pub fn acquire(
    identity: &EntryIdentity,
    cache: &Cache,
    options: &AcquireOptions,
) -> std::result::Result<Acquired, EntryError> {
    if let EntryIdentity::Path { spec, resolved } = identity {
        if !resolved.is_dir() {
            return Err(EntryError::acquisition(format!(
                "local entry is not a directory: {spec}"
            )));
        }
        return Ok(Acquired {
            root: resolved.clone(),
            from_cache: false,
        });
    }
    let destination = cache.entry_directory(identity).ok_or_else(|| {
        EntryError::acquisition("no cache location is defined for this entry".to_owned())
    })?;
    if options.refresh {
        cache::discard(&destination).map_err(|error| {
            EntryError::acquisition(format!("failed to discard the cached entry: {error}"))
        })?;
    } else if cache::is_complete(&destination) {
        return Ok(Acquired {
            root: destination,
            from_cache: true,
        });
    } else {
        cache::discard(&destination).map_err(|error| {
            EntryError::acquisition(format!("failed to discard a partial cached entry: {error}"))
        })?;
    }
    let deadline = Instant::now() + options.timeout;
    with_retries(deadline, || match identity {
        EntryIdentity::Npm { package, version } => {
            acquire_npm(package, version, &destination, options, deadline)
        }
        EntryIdentity::Git { url, commit } => acquire_git(url, commit, &destination, deadline),
        EntryIdentity::Path { .. } => Err(Retryable::fatal("unreachable local entry")),
    })
    .map_err(EntryError::acquisition)?;
    cache::mark_complete(&destination).map_err(|error| {
        EntryError::acquisition(format!("failed to mark the cached entry complete: {error}"))
    })?;
    Ok(Acquired {
        root: destination,
        from_cache: false,
    })
}

/// A failure that either may or may not be worth retrying.
#[derive(Debug)]
pub struct Retryable {
    message: String,
    retry: bool,
}

impl Retryable {
    fn fatal(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            retry: false,
        }
    }

    fn transport(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            retry: true,
        }
    }
}

fn with_retries(
    deadline: Instant,
    mut attempt: impl FnMut() -> std::result::Result<(), Retryable>,
) -> std::result::Result<(), String> {
    let mut backoff = Duration::from_secs(1);
    for remaining in (0..=RETRIES).rev() {
        match attempt() {
            Ok(()) => return Ok(()),
            Err(failure) if remaining == 0 || !failure.retry => return Err(failure.message),
            Err(_) => {
                if Instant::now() + backoff >= deadline {
                    return Err("acquisition timed out".to_owned());
                }
                thread::sleep(backoff);
                backoff *= 2;
            }
        }
    }
    Err("acquisition failed".to_owned())
}

fn acquire_npm(
    package: &str,
    version: &str,
    destination: &Path,
    options: &AcquireOptions,
    deadline: Instant,
) -> std::result::Result<(), Retryable> {
    let agent = agent(deadline)?;
    let url = format!(
        "{}/{}/{version}",
        options.registry.trim_end_matches('/'),
        package.replace('/', "%2f")
    );
    let metadata = fetch(&agent, &url, METADATA_LIMIT)?;
    let metadata: serde_json::Value = serde_json::from_slice(&metadata).map_err(|error| {
        Retryable::fatal(format!("registry metadata is not valid JSON: {error}"))
    })?;
    let tarball = metadata["dist"]["tarball"].as_str().ok_or_else(|| {
        Retryable::fatal("registry metadata does not contain dist.tarball".to_owned())
    })?;
    let expected = metadata["dist"]["integrity"].as_str().unwrap_or_default();
    let bytes = fetch(&agent, tarball, TARBALL_LIMIT)?;
    integrity::verify(&bytes, expected).map_err(Retryable::fatal)?;

    let staging = staging_path(destination);
    let _ = cache::discard(&staging);
    archive::extract_tar_gz(&bytes, &staging, 1).map_err(|error| {
        let _ = cache::discard(&staging);
        Retryable::fatal(error)
    })?;
    promote(&staging, destination)
}

fn acquire_git(
    url: &str,
    commit: &str,
    destination: &Path,
    deadline: Instant,
) -> std::result::Result<(), Retryable> {
    let staging = staging_path(destination);
    let _ = cache::discard(&staging);
    fs::create_dir_all(&staging)
        .map_err(|error| Retryable::transport(format!("failed to create {staging:?}: {error}")))?;

    // A shallow fetch of the exact commit, where the server allows it.
    let shallow = git(&["init", "--quiet"], &staging, deadline)
        .and_then(|_| git(&["remote", "add", "origin", url], &staging, deadline))
        .and_then(|_| {
            git(
                &["fetch", "--depth", "1", "--no-tags", "origin", commit],
                &staging,
                deadline,
            )
        })
        .and_then(|_| {
            git(
                &["checkout", "--detach", "--force", "FETCH_HEAD"],
                &staging,
                deadline,
            )
        });
    if shallow.is_err() {
        // Fall back to a full clone plus checkout. Submodules are never
        // fetched.
        let _ = cache::discard(&staging);
        let target = staging.to_string_lossy().into_owned();
        git(
            &[
                "clone",
                "--no-checkout",
                "--recurse-submodules=no",
                url,
                &target,
            ],
            Path::new("."),
            deadline,
        )?;
        git(
            &["checkout", "--detach", "--force", commit],
            &staging,
            deadline,
        )?;
    }
    promote(&staging, destination)
}

fn staging_path(destination: &Path) -> PathBuf {
    let name = destination
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "entry".into());
    destination.with_file_name(format!(".{name}.incoming-{}", std::process::id()))
}

fn promote(staging: &Path, destination: &Path) -> std::result::Result<(), Retryable> {
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            Retryable::fatal(format!("failed to create {}: {error}", parent.display()))
        })?;
    }
    let _ = cache::discard(destination);
    fs::rename(staging, destination).map_err(|error| {
        let _ = cache::discard(staging);
        Retryable::fatal(format!(
            "failed to move the acquired entry into the cache: {error}"
        ))
    })
}

fn agent(deadline: Instant) -> std::result::Result<ureq::Agent, Retryable> {
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        return Err(Retryable::fatal("acquisition timed out"));
    }
    let config = ureq::Agent::config_builder()
        .timeout_global(Some(remaining))
        .user_agent(concat!("concord/", env!("CARGO_PKG_VERSION")))
        .build();
    Ok(config.into())
}

fn fetch(agent: &ureq::Agent, url: &str, limit: u64) -> std::result::Result<Vec<u8>, Retryable> {
    let mut response = agent.get(url).call().map_err(|error| match &error {
        ureq::Error::StatusCode(code) if (400..500).contains(code) && *code != 429 => {
            Retryable::fatal(format!("{url} returned HTTP {code}"))
        }
        _ => Retryable::transport(format!("{url}: {error}")),
    })?;
    response
        .body_mut()
        .with_config()
        .limit(limit)
        .read_to_vec()
        .map_err(|error| Retryable::transport(format!("failed to read {url}: {error}")))
}

/// Run one git command with a deadline, killing the whole process group on
/// timeout so that nothing is left behind.
fn git(
    arguments: &[&str],
    directory: &Path,
    deadline: Instant,
) -> std::result::Result<(), Retryable> {
    let mut command = Command::new("git");
    command
        .args(arguments)
        .current_dir(directory)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_ASKPASS", "")
        .env("GIT_LFS_SKIP_SMUDGE", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.group_spawn().map_err(|error| {
        Retryable::fatal(format!("failed to start git (is it installed?): {error}"))
    })?;
    let stdout = child.inner().stdout.take();
    let stderr = child.inner().stderr.take();
    let stdout_reader = thread::spawn(move || read_all(stdout));
    let stderr_reader = thread::spawn(move || read_all(stderr));
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(20)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = stdout_reader.join();
                let _ = stderr_reader.join();
                return Err(Retryable::transport(format!(
                    "git {} timed out",
                    arguments.join(" ")
                )));
            }
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(Retryable::transport(format!("git failed: {error}")));
            }
        }
    };
    let _ = stdout_reader.join();
    let stderr = stderr_reader.join().unwrap_or_default();
    if status.success() {
        return Ok(());
    }
    Err(Retryable::transport(format!(
        "git {} failed with {:?}\n{}",
        arguments.join(" "),
        status.code(),
        crate::process::truncate(&String::from_utf8_lossy(&stderr), 2_048)
    )))
}

fn read_all<R: Read>(reader: Option<R>) -> Vec<u8> {
    let mut output = Vec::new();
    if let Some(mut reader) = reader {
        let _ = reader.read_to_end(&mut output);
    }
    output
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};
    use std::time::{Duration, Instant};

    use tempfile::tempdir;

    use super::{AcquireOptions, Retryable, acquire, staging_path, with_retries};
    use crate::corpus::cache::Cache;
    use crate::corpus::manifest::EntryIdentity;

    fn options() -> AcquireOptions {
        AcquireOptions {
            registry: super::DEFAULT_REGISTRY.into(),
            timeout: Duration::from_secs(120),
            refresh: false,
        }
    }

    #[test]
    fn local_entries_are_read_in_place_and_never_cached() {
        let directory = tempdir().expect("tempdir");
        let cache_directory = tempdir().expect("cache tempdir");
        let cache = Cache::open(Some(cache_directory.path())).expect("cache");
        let acquired = acquire(
            &EntryIdentity::Path {
                spec: "./project".into(),
                resolved: directory.path().to_path_buf(),
            },
            &cache,
            &options(),
        )
        .expect("local acquisition");
        assert_eq!(acquired.root, directory.path());
        assert!(!acquired.from_cache);
        assert_eq!(
            std::fs::read_dir(cache_directory.path())
                .expect("cache directory")
                .count(),
            0,
            "a local entry must not be copied into the cache"
        );
    }

    #[test]
    fn a_missing_local_directory_is_an_acquisition_error() {
        let cache_directory = tempdir().expect("cache tempdir");
        let cache = Cache::open(Some(cache_directory.path())).expect("cache");
        let error = acquire(
            &EntryIdentity::Path {
                spec: "./missing".into(),
                resolved: PathBuf::from("/definitely/missing/concord-corpus"),
            },
            &cache,
            &options(),
        )
        .expect_err("missing directory");
        assert_eq!(
            error.kind,
            crate::corpus::entry::EntryErrorKind::Acquisition
        );
        assert!(error.message.contains("not a directory"));
    }

    #[test]
    fn transport_failures_retry_and_fatal_failures_do_not() {
        let deadline = Instant::now() + Duration::from_secs(30);
        let mut transport = 0;
        let error = with_retries(deadline, || {
            transport += 1;
            Err(Retryable::transport("connection reset"))
        })
        .expect_err("exhausted");
        assert_eq!(transport, 3, "one attempt plus two retries");
        assert_eq!(error, "connection reset");

        let mut fatal = 0;
        let error = with_retries(deadline, || {
            fatal += 1;
            Err(Retryable::fatal("HTTP 404"))
        })
        .expect_err("fatal");
        assert_eq!(fatal, 1, "a 404 is never retried");
        assert_eq!(error, "HTTP 404");

        let mut recovering = 0;
        with_retries(deadline, || {
            recovering += 1;
            if recovering < 2 {
                Err(Retryable::transport("temporary"))
            } else {
                Ok(())
            }
        })
        .expect("recovered");
        assert_eq!(recovering, 2);
    }

    #[test]
    fn a_retry_never_outlives_the_acquisition_deadline() {
        let error = with_retries(Instant::now(), || Err(Retryable::transport("slow")))
            .expect_err("deadline");
        assert_eq!(error, "acquisition timed out");
    }

    #[test]
    fn staging_is_a_sibling_of_the_final_directory() {
        let staging = staging_path(Path::new("/cache/npm/lodash/4.17.21"));
        assert_eq!(staging.parent(), Path::new("/cache/npm/lodash").into());
        assert_ne!(staging.file_name(), Path::new("4.17.21").file_name());
    }
}
