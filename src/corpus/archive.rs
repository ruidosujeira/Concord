//! Guarded extraction of npm tarballs.
//!
//! Corpus entries are untrusted. Nothing from an archive is executed, no
//! permissions or extended attributes are carried over, and any member that
//! would land outside the destination directory is rejected.

use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

use flate2::read::GzDecoder;
use tar::{Archive, EntryType};

/// Bounds that keep a hostile archive from exhausting the machine.
const MAX_ENTRIES: usize = 100_000;
const MAX_TOTAL_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const MAX_FILE_BYTES: u64 = 256 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtractionSummary {
    pub files: usize,
    pub bytes: u64,
}

/// Extract a gzip-compressed tar into `destination`, dropping `strip` leading
/// path components from every member. npm tarballs place everything under a
/// single `package/` directory, so callers strip one component.
pub fn extract_tar_gz(
    bytes: &[u8],
    destination: &Path,
    strip: usize,
) -> std::result::Result<ExtractionSummary, String> {
    fs::create_dir_all(destination)
        .map_err(|error| format!("failed to create {}: {error}", destination.display()))?;
    let mut archive = Archive::new(GzDecoder::new(bytes));
    let entries = archive
        .entries()
        .map_err(|error| format!("failed to read the archive: {error}"))?;
    let mut summary = ExtractionSummary { files: 0, bytes: 0 };
    let mut seen = 0usize;
    for entry in entries {
        let mut entry =
            entry.map_err(|error| format!("failed to read an archive member: {error}"))?;
        seen += 1;
        if seen > MAX_ENTRIES {
            return Err(format!(
                "the archive contains more than {MAX_ENTRIES} members"
            ));
        }
        let raw = entry
            .path()
            .map_err(|error| format!("an archive member has an unreadable path: {error}"))?
            .into_owned();
        let Some(relative) = safe_relative_path(&raw, strip)? else {
            continue;
        };
        let target = destination.join(&relative);
        if !target.starts_with(destination) {
            return Err(escape(&raw));
        }
        match entry.header().entry_type() {
            EntryType::Directory => {
                fs::create_dir_all(&target)
                    .map_err(|error| format!("failed to create {}: {error}", relative.display()))?;
            }
            EntryType::Regular | EntryType::Continuous => {
                let size = entry.header().size().unwrap_or(0);
                if size > MAX_FILE_BYTES {
                    return Err(format!(
                        "archive member {} is larger than {MAX_FILE_BYTES} bytes",
                        relative.display()
                    ));
                }
                summary.bytes += size;
                if summary.bytes > MAX_TOTAL_BYTES {
                    return Err(format!(
                        "the archive expands to more than {MAX_TOTAL_BYTES} bytes"
                    ));
                }
                if let Some(parent) = target.parent() {
                    fs::create_dir_all(parent).map_err(|error| {
                        format!("failed to create {}: {error}", parent.display())
                    })?;
                }
                let mut contents = Vec::new();
                entry
                    .read_to_end(&mut contents)
                    .map_err(|error| format!("failed to read {}: {error}", relative.display()))?;
                fs::write(&target, &contents)
                    .map_err(|error| format!("failed to write {}: {error}", relative.display()))?;
                summary.files += 1;
            }
            // Links, devices and fifos are never needed to read a project as
            // data, and a link is the classic way out of the destination.
            _ => continue,
        }
    }
    Ok(summary)
}

/// Reject absolute paths, drive prefixes and any `..`, then strip the leading
/// components. `Ok(None)` means the member is the stripped prefix itself.
fn safe_relative_path(path: &Path, strip: usize) -> std::result::Result<Option<PathBuf>, String> {
    let mut parts: Vec<&std::ffi::OsStr> = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => parts.push(part),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(escape(path));
            }
        }
    }
    // A backslash is a separator on Windows only, so normalize it everywhere
    // before deciding whether a member escapes.
    if parts.iter().any(|part| {
        part.to_string_lossy()
            .split('\\')
            .any(|piece| piece == "..")
    }) {
        return Err(escape(path));
    }
    if parts.len() <= strip {
        return Ok(None);
    }
    Ok(Some(parts[strip..].iter().collect()))
}

fn escape(path: &Path) -> String {
    format!(
        "archive member {} would be written outside the destination directory",
        path.display()
    )
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::io::Write;
    use std::path::Path;

    use flate2::Compression;
    use flate2::write::GzEncoder;
    use tempfile::tempdir;

    use super::extract_tar_gz;

    /// Build a tar member with a literal name, bypassing the writer's own path
    /// validation so that hostile names can be tested.
    fn member(builder: &mut tar::Builder<Vec<u8>>, name: &str, data: &[u8]) {
        let mut header = tar::Header::new_ustar();
        let bytes = name.as_bytes();
        let old = header.as_old_mut();
        old.name[..bytes.len()].copy_from_slice(bytes);
        header.set_size(data.len() as u64);
        header.set_mode(0o644);
        header.set_mtime(0);
        header.set_entry_type(tar::EntryType::Regular);
        header.set_cksum();
        builder.append(&header, data).expect("append member");
    }

    fn archive(members: &[(&str, &[u8])]) -> Vec<u8> {
        let mut builder = tar::Builder::new(Vec::new());
        for (name, data) in members {
            member(&mut builder, name, data);
        }
        let tar = builder.into_inner().expect("tar bytes");
        let mut encoder = GzEncoder::new(Vec::new(), Compression::fast());
        encoder.write_all(&tar).expect("compress");
        encoder.finish().expect("gzip bytes")
    }

    fn outside(directory: &Path) -> Vec<String> {
        fs::read_dir(directory)
            .expect("read outer directory")
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| name != "destination")
            .collect()
    }

    #[test]
    fn extracts_regular_files_and_strips_the_package_prefix() {
        let directory = tempdir().expect("tempdir");
        let destination = directory.path().join("destination");
        let bytes = archive(&[
            ("package/src/app.ts", b"const value = 1;\n"),
            ("package/package.json", b"{}\n"),
        ]);
        let summary = extract_tar_gz(&bytes, &destination, 1).expect("extraction");
        assert_eq!(summary.files, 2);
        assert_eq!(
            fs::read_to_string(destination.join("src/app.ts")).expect("extracted file"),
            "const value = 1;\n"
        );
    }

    #[test]
    fn a_traversing_member_is_rejected_without_writing_outside() {
        let directory = tempdir().expect("tempdir");
        let destination = directory.path().join("destination");
        for name in [
            "package/../escape",
            "../escape",
            "package/nested/../../../escape",
        ] {
            let bytes = archive(&[(name, b"owned\n")]);
            let error = extract_tar_gz(&bytes, &destination, 1).expect_err("rejected");
            assert!(error.contains("outside the destination"), "{error}");
            assert!(
                outside(directory.path()).is_empty(),
                "wrote outside for {name}"
            );
            assert!(!directory.path().join("escape").exists());
        }
    }

    #[test]
    fn an_absolute_member_is_rejected_without_writing_outside() {
        let directory = tempdir().expect("tempdir");
        let destination = directory.path().join("destination");
        let absolute = directory.path().join("escape");
        let bytes = archive(&[
            ("/etc/concord-escape", b"owned\n" as &[u8]),
            (
                absolute.to_str().expect("utf-8 temp path"),
                b"owned\n" as &[u8],
            ),
        ]);
        let error = extract_tar_gz(&bytes, &destination, 1).expect_err("rejected");
        assert!(error.contains("outside the destination"), "{error}");
        assert!(!absolute.exists());
        assert!(!Path::new("/etc/concord-escape").exists());
        assert!(outside(directory.path()).is_empty());
    }

    #[test]
    fn links_and_devices_are_skipped() {
        let directory = tempdir().expect("tempdir");
        let destination = directory.path().join("destination");
        let mut builder = tar::Builder::new(Vec::new());
        let mut header = tar::Header::new_ustar();
        let name = b"package/link";
        header.as_old_mut().name[..name.len()].copy_from_slice(name);
        header.set_size(0);
        header.set_mode(0o777);
        header.set_mtime(0);
        header.set_entry_type(tar::EntryType::Symlink);
        header.set_link_name("/etc/passwd").expect("symlink target");
        header.set_cksum();
        builder.append(&header, &[][..]).expect("append symlink");
        member(&mut builder, "package/real.ts", b"ok\n");
        let tar = builder.into_inner().expect("tar bytes");
        let mut encoder = GzEncoder::new(Vec::new(), Compression::fast());
        encoder.write_all(&tar).expect("compress");
        let bytes = encoder.finish().expect("gzip bytes");

        let summary = extract_tar_gz(&bytes, &destination, 1).expect("extraction");
        assert_eq!(summary.files, 1);
        assert!(!destination.join("link").exists());
        assert!(destination.join("real.ts").is_file());
    }
}
