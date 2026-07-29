use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use globset::{Glob, GlobSet, GlobSetBuilder};
use ignore::WalkBuilder;

use crate::config::DiscoveryConfig;
use crate::error::{ConcordError, Result};
use crate::model::normalize_path;

const DEFAULT_IGNORED_DIRS: [&str; 8] = [
    ".git",
    "node_modules",
    "target",
    "dist",
    "build",
    "coverage",
    ".concord",
    ".svn",
];

pub fn discover(root: &Path, inputs: &[PathBuf], config: &DiscoveryConfig) -> Result<Vec<PathBuf>> {
    let includes = build_globs(&config.include, "discovery.include")?;
    let excludes = build_globs(&config.exclude, "discovery.exclude")?;
    let targets = if inputs.is_empty() {
        vec![root.to_path_buf()]
    } else {
        inputs
            .iter()
            .map(|input| {
                if input.is_absolute() {
                    input.clone()
                } else {
                    root.join(input)
                }
            })
            .collect()
    };
    let mut files = BTreeSet::new();
    for target in targets {
        if !target.exists() {
            return Err(ConcordError::usage(format!(
                "input path does not exist: {}",
                target.display()
            )));
        }
        if target.is_file() {
            let normalized = normalize_path(root, &target);
            if !is_supported(&normalized, &includes, &excludes) {
                return Err(ConcordError::usage(format!(
                    "unsupported or excluded file: {normalized}"
                )));
            }
            files.insert((normalized, target));
            continue;
        }

        let walk_root = target.clone();
        let mut builder = WalkBuilder::new(&target);
        builder
            .standard_filters(true)
            .hidden(false)
            .follow_links(false);
        builder.filter_entry(move |entry| {
            entry.path() == walk_root
                || !entry.path().components().any(|component| {
                    DEFAULT_IGNORED_DIRS.contains(&component.as_os_str().to_string_lossy().as_ref())
                })
        });
        for entry in builder.build() {
            let entry = entry.map_err(|error| {
                ConcordError::operational(format!(
                    "failed while discovering files under {}\nerror: {error}",
                    target.display()
                ))
            })?;
            if !entry
                .file_type()
                .is_some_and(|file_type| file_type.is_file())
            {
                continue;
            }
            let normalized = normalize_path(root, entry.path());
            if is_supported(&normalized, &includes, &excludes) {
                files.insert((normalized, entry.into_path()));
            }
        }
    }
    Ok(files.into_iter().map(|(_, path)| path).collect())
}

fn build_globs(patterns: &[String], field: &str) -> Result<GlobSet> {
    let mut builder = GlobSetBuilder::new();
    for pattern in patterns {
        let glob = Glob::new(pattern).map_err(|error| {
            ConcordError::usage(format!(
                "invalid glob in {field}: `{pattern}`\nerror: {error}"
            ))
        })?;
        builder.add(glob);
    }
    builder.build().map_err(|error| {
        ConcordError::usage(format!("failed to compile globs in {field}: {error}"))
    })
}

fn is_supported(path: &str, includes: &GlobSet, excludes: &GlobSet) -> bool {
    includes.is_match(path) && !excludes.is_match(path)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::discover;
    use crate::config::DiscoveryConfig;

    #[test]
    fn discovers_supported_files_and_ignores_defaults() {
        let directory = tempdir().expect("tempdir");
        fs::create_dir_all(directory.path().join("src")).expect("src");
        fs::create_dir_all(directory.path().join("node_modules/pkg")).expect("node_modules");
        fs::write(directory.path().join("src/app.ts"), "").expect("source");
        fs::write(directory.path().join("src/readme.md"), "").expect("readme");
        fs::write(directory.path().join("node_modules/pkg/index.js"), "").expect("dependency");
        let files = discover(directory.path(), &[], &DiscoveryConfig::default()).expect("discover");
        assert_eq!(files, vec![directory.path().join("src/app.ts")]);
    }

    #[test]
    fn respects_gitignore() {
        let directory = tempdir().expect("tempdir");
        fs::create_dir(directory.path().join(".git")).expect("git directory");
        fs::write(directory.path().join(".gitignore"), "ignored.ts\n").expect("gitignore");
        fs::write(directory.path().join("ignored.ts"), "").expect("ignored source");
        fs::write(directory.path().join("included.ts"), "").expect("included source");
        let files = discover(directory.path(), &[], &DiscoveryConfig::default()).expect("discover");
        assert_eq!(files, vec![directory.path().join("included.ts")]);
    }
}
