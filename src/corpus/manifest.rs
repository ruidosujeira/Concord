//! Corpus manifest parsing.
//!
//! A manifest is plain UTF-8 text with one entry per line. Blank lines and
//! lines whose first non-whitespace character is `#` are ignored. Every entry
//! carries a required scheme prefix and is fully pinned.

use std::fmt;
use std::fs;
use std::path::{Component, Path, PathBuf};

use serde::{Serialize, Serializer};
use sha2::{Digest, Sha256};

use crate::error::{ConcordError, Result};

const MAX_PACKAGE_NAME: usize = 214;
const COMMIT_LENGTH: usize = 40;

/// The pinned identity of one corpus entry.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum EntryIdentity {
    Npm {
        package: String,
        version: String,
    },
    Git {
        url: String,
        commit: String,
    },
    Path {
        /// The path exactly as written in the manifest.
        spec: String,
        /// `spec` resolved against the manifest file's own directory.
        resolved: PathBuf,
    },
}

impl EntryIdentity {
    /// The entry rendered back in manifest form. This is the only project
    /// identity that ever reaches a report.
    pub fn manifest_form(&self) -> String {
        match self {
            Self::Npm { package, version } => format!("npm:{package}@{version}"),
            Self::Git { url, commit } => format!("git:{url}#{commit}"),
            Self::Path { spec, .. } => format!("path:{spec}"),
        }
    }

    /// Two entries with the same key denote the same project. Local entries
    /// compare by resolved location so that different spellings of one
    /// directory collapse.
    pub fn dedup_key(&self) -> String {
        match self {
            Self::Path { resolved, .. } => format!("path:{}", resolved.to_string_lossy()),
            other => other.manifest_form(),
        }
    }

    pub fn scheme(&self) -> &'static str {
        match self {
            Self::Npm { .. } => "npm",
            Self::Git { .. } => "git",
            Self::Path { .. } => "path",
        }
    }

    /// Local entries are read in place and are never copied or cached.
    pub fn is_local(&self) -> bool {
        matches!(self, Self::Path { .. })
    }
}

impl fmt::Display for EntryIdentity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.manifest_form())
    }
}

impl Serialize for EntryIdentity {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.manifest_form())
    }
}

#[derive(Debug, Clone)]
pub struct ManifestEntry {
    pub identity: EntryIdentity,
    /// 1-based line number in the manifest.
    pub line: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManifestParseError {
    pub line: usize,
    pub text: String,
    pub expected: String,
}

#[derive(Debug, Clone)]
pub struct Manifest {
    pub path: PathBuf,
    pub entries: Vec<ManifestEntry>,
    /// Collapsed-duplicate notices, reported to stderr by the caller.
    pub warnings: Vec<String>,
}

impl Manifest {
    /// A canonical, order-preserving rendering of the resolved entries. This
    /// feeds the resume key, so it must change whenever the corpus changes.
    pub fn resolved_source(&self) -> String {
        self.entries
            .iter()
            .map(|entry| entry.identity.dedup_key())
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub fn digest(&self) -> String {
        format!("{:x}", Sha256::digest(self.resolved_source().as_bytes()))
    }
}

/// Read and parse a manifest. Every parse error in the file is collected and
/// reported together.
pub fn load(path: &Path) -> Result<Manifest> {
    let bytes = fs::read(path).map_err(|error| {
        ConcordError::run_failure(format!(
            "failed to read corpus manifest\npath: {}\nerror: {error}",
            path.display()
        ))
    })?;
    let text = String::from_utf8(bytes).map_err(|error| {
        ConcordError::run_failure(format!(
            "corpus manifest is not valid UTF-8\npath: {}\nerror: {error}",
            path.display()
        ))
    })?;
    let directory = path.parent().unwrap_or_else(|| Path::new("."));
    match parse(&text, directory) {
        Ok(mut manifest) => {
            manifest.path = path.to_path_buf();
            if manifest.entries.is_empty() {
                return Err(ConcordError::run_failure(format!(
                    "corpus manifest contains no entries\npath: {}",
                    path.display()
                )));
            }
            Ok(manifest)
        }
        Err(errors) => Err(ConcordError::run_failure(render_errors(path, &errors))),
    }
}

/// Parse manifest text. `directory` is the manifest file's own directory;
/// `path:` entries resolve against it, never against the process working
/// directory.
pub fn parse(
    text: &str,
    directory: &Path,
) -> std::result::Result<Manifest, Vec<ManifestParseError>> {
    let mut entries: Vec<ManifestEntry> = Vec::new();
    let mut errors = Vec::new();
    let mut warnings = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    for (index, raw) in text.lines().enumerate() {
        let line = index + 1;
        let trimmed = raw.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        match parse_entry(trimmed, directory) {
            Ok(identity) => {
                let key = identity.dedup_key();
                if seen.contains(&key) {
                    warnings.push(format!(
                        "duplicate corpus entry collapsed: {} (line {line})",
                        identity.manifest_form()
                    ));
                    continue;
                }
                seen.push(key);
                entries.push(ManifestEntry { identity, line });
            }
            Err(expected) => errors.push(ManifestParseError {
                line,
                text: trimmed.to_owned(),
                expected,
            }),
        }
    }
    if errors.is_empty() {
        Ok(Manifest {
            path: directory.join("<memory>"),
            entries,
            warnings,
        })
    } else {
        Err(errors)
    }
}

fn render_errors(path: &Path, errors: &[ManifestParseError]) -> String {
    let mut message = format!(
        "invalid corpus manifest\npath: {}\n{} invalid line(s):",
        path.display(),
        errors.len()
    );
    for error in errors {
        message.push_str(&format!(
            "\n  line {}: {}\n    expected {}",
            error.line, error.text, error.expected
        ));
    }
    message
}

fn parse_entry(line: &str, directory: &Path) -> std::result::Result<EntryIdentity, String> {
    let Some((scheme, rest)) = line.split_once(':') else {
        return Err(scheme_help());
    };
    match scheme {
        "npm" => parse_npm(rest),
        "git" => parse_git(rest),
        "path" => parse_path(rest, directory),
        _ => Err(scheme_help()),
    }
}

fn scheme_help() -> String {
    "one of npm:<package-name>@<exact-version>, \
     git:<https-url>#<40-character-commit-sha>, or path:<directory>"
        .into()
}

fn parse_npm(rest: &str) -> std::result::Result<EntryIdentity, String> {
    let npm_help = "npm:<package-name>@<exact-version>, fully pinned \
                    (a range, a dist-tag, or an omitted version is not accepted)";
    // A scoped name starts with `@`, so only a later `@` separates the version.
    let separator = rest
        .char_indices()
        .rfind(|(index, character)| *character == '@' && *index > 0)
        .map(|(index, _)| index);
    let Some(separator) = separator else {
        return Err(npm_help.into());
    };
    let package = &rest[..separator];
    let version = &rest[separator + 1..];
    if !is_valid_package_name(package) {
        return Err(format!("a valid npm package name; {npm_help}"));
    }
    if !is_exact_version(version) {
        return Err(format!("an exact version such as 1.2.3; {npm_help}"));
    }
    Ok(EntryIdentity::Npm {
        package: package.to_owned(),
        version: version.to_owned(),
    })
}

fn parse_git(rest: &str) -> std::result::Result<EntryIdentity, String> {
    let git_help = "git:<https-url>#<40-character-commit-sha> \
                    (a branch, tag, short sha, or omitted ref is not accepted)";
    let Some((url, commit)) = rest.rsplit_once('#') else {
        return Err(git_help.into());
    };
    if !url.starts_with("https://") || url.len() <= "https://".len() {
        return Err(format!("an https:// repository URL; {git_help}"));
    }
    if commit.len() != COMMIT_LENGTH || !commit.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(format!("a full 40-character commit sha; {git_help}"));
    }
    Ok(EntryIdentity::Git {
        url: url.to_owned(),
        commit: commit.to_ascii_lowercase(),
    })
}

fn parse_path(rest: &str, directory: &Path) -> std::result::Result<EntryIdentity, String> {
    if rest.is_empty() {
        return Err("path:<directory>, absolute or relative to the manifest file".into());
    }
    let candidate = Path::new(rest);
    let resolved = if candidate.is_absolute() {
        lexical_absolute(candidate)
    } else {
        lexical_absolute(&directory.join(candidate))
    };
    Ok(EntryIdentity::Path {
        spec: rest.to_owned(),
        resolved,
    })
}

/// Normalize `.` and `..` textually. The directory does not have to exist yet;
/// a missing directory is an acquisition failure, not a parse error.
fn lexical_absolute(path: &Path) -> PathBuf {
    let mut result = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !result.pop() {
                    result.push("..");
                }
            }
            other => result.push(other.as_os_str()),
        }
    }
    result
}

fn is_valid_package_name(name: &str) -> bool {
    if name.is_empty() || name.len() > MAX_PACKAGE_NAME {
        return false;
    }
    let unscoped = if let Some(scoped) = name.strip_prefix('@') {
        let Some((scope, remainder)) = scoped.split_once('/') else {
            return false;
        };
        if !is_valid_name_segment(scope) {
            return false;
        }
        remainder
    } else {
        name
    };
    is_valid_name_segment(unscoped)
}

fn is_valid_name_segment(segment: &str) -> bool {
    !segment.is_empty()
        && segment != "."
        && segment != ".."
        && !segment.starts_with('.')
        && !segment.starts_with('_')
        && segment
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '-' | '_' | '.'))
}

/// Accept only a complete semantic version. Anything that could denote more
/// than one published artifact is rejected.
fn is_exact_version(version: &str) -> bool {
    let (without_build, build) = match version.split_once('+') {
        Some((left, right)) => (left, Some(right)),
        None => (version, None),
    };
    let (core, prerelease) = match without_build.split_once('-') {
        Some((left, right)) => (left, Some(right)),
        None => (without_build, None),
    };
    let numbers: Vec<&str> = core.split('.').collect();
    if numbers.len() != 3 || !numbers.iter().all(|part| is_numeric_identifier(part)) {
        return false;
    }
    [prerelease, build]
        .into_iter()
        .flatten()
        .all(is_dot_separated_identifier)
}

fn is_numeric_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.chars().all(|c| c.is_ascii_digit())
        && (value == "0" || !value.starts_with('0'))
}

fn is_dot_separated_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.split('.').all(|part| {
            !part.is_empty() && part.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        })
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::{EntryIdentity, parse};

    fn identities(text: &str, directory: &Path) -> Vec<EntryIdentity> {
        parse(text, directory)
            .expect("manifest parses")
            .entries
            .into_iter()
            .map(|entry| entry.identity)
            .collect()
    }

    #[test]
    fn parses_all_three_entry_forms() {
        let directory = Path::new("/manifests");
        let text = "npm:lodash@4.17.21\n\
                    git:https://github.com/acme/repo#0123456789abcdef0123456789abcdef01234567\n\
                    path:projects/one\n";
        assert_eq!(
            identities(text, directory),
            vec![
                EntryIdentity::Npm {
                    package: "lodash".into(),
                    version: "4.17.21".into()
                },
                EntryIdentity::Git {
                    url: "https://github.com/acme/repo".into(),
                    commit: "0123456789abcdef0123456789abcdef01234567".into()
                },
                EntryIdentity::Path {
                    spec: "projects/one".into(),
                    resolved: PathBuf::from("/manifests/projects/one")
                },
            ]
        );
    }

    #[test]
    fn parses_scoped_names_comments_blanks_and_whitespace() {
        let text = "# a comment\n\
                    \n\
                       \t\n\
                    \t# indented comment\n\
                      npm:@scope/name@1.2.3-alpha.1+build.5   \n";
        let entries = identities(text, Path::new("/manifests"));
        assert_eq!(
            entries,
            vec![EntryIdentity::Npm {
                package: "@scope/name".into(),
                version: "1.2.3-alpha.1+build.5".into()
            }]
        );
        assert_eq!(
            entries[0].manifest_form(),
            "npm:@scope/name@1.2.3-alpha.1+build.5"
        );
    }

    #[test]
    fn duplicates_collapse_with_a_warning() {
        let text = "path:./one\n\
                    path:one\n\
                    npm:lodash@4.17.21\n\
                    npm:lodash@4.17.21\n";
        let manifest = parse(text, Path::new("/manifests")).expect("manifest parses");
        assert_eq!(manifest.entries.len(), 2);
        assert_eq!(manifest.warnings.len(), 2);
        assert!(manifest.warnings[0].contains("duplicate corpus entry collapsed"));
        assert!(manifest.warnings[0].contains("line 2"));
    }

    #[test]
    fn path_entries_resolve_against_the_manifest_directory() {
        let entries = identities("path:../sibling/project\n", Path::new("/a/b/manifests"));
        assert_eq!(
            entries,
            vec![EntryIdentity::Path {
                spec: "../sibling/project".into(),
                resolved: PathBuf::from("/a/b/sibling/project")
            }]
        );
    }

    #[test]
    fn every_bad_line_is_reported_together_with_its_number() {
        let text = "npm:lodash\n\
                    npm:lodash@^4.0.0\n\
                    npm:lodash@latest\n\
                    git:https://github.com/acme/repo#main\n\
                    git:https://github.com/acme/repo#0123456\n\
                    git:https://github.com/acme/repo\n\
                    ftp://example.com/thing\n\
                    lodash@4.17.21\n\
                    npm:@scope/name\n";
        let errors = parse(text, Path::new("/manifests")).expect_err("every line is invalid");
        assert_eq!(
            errors.iter().map(|error| error.line).collect::<Vec<_>>(),
            vec![1, 2, 3, 4, 5, 6, 7, 8, 9]
        );
        assert_eq!(errors[0].text, "npm:lodash");
        assert!(errors[1].expected.contains("exact version"));
        assert!(errors[3].expected.contains("40-character commit sha"));
        assert!(errors[6].expected.contains("npm:<package-name>"));
        assert!(errors[7].expected.contains("npm:<package-name>"));
    }

    #[test]
    fn an_empty_manifest_parses_to_no_entries() {
        let manifest = parse("\n# only comments\n\n", Path::new("/manifests"))
            .expect("an empty manifest is not a parse error");
        assert!(manifest.entries.is_empty());
    }

    #[test]
    fn unpinned_and_malformed_versions_are_rejected() {
        for version in ["1", "1.2", "1.2.x", "*", "1.2.3.4", "01.2.3", "1.2.3-", ""] {
            let text = format!("npm:pkg@{version}\n");
            assert!(
                parse(&text, Path::new("/manifests")).is_err(),
                "accepted unpinned version {version:?}"
            );
        }
        assert!(parse("npm:pkg@1.2.3\n", Path::new("/m")).is_ok());
    }

    #[test]
    fn traversal_and_separator_characters_are_rejected_in_package_names() {
        for name in ["../evil", "UPPER", "a/b", "@scope", ".hidden", "_leading"] {
            let text = format!("npm:{name}@1.0.0\n");
            assert!(
                parse(&text, Path::new("/manifests")).is_err(),
                "accepted package name {name:?}"
            );
        }
    }
}
