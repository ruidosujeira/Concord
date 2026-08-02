use std::ffi::OsString;
use std::path::Path;

pub fn arguments(path: &Path) -> Vec<OsString> {
    vec![
        OsString::from("--stdin-filepath"),
        path.as_os_str().to_owned(),
    ]
}

pub fn runtime_unsupported_reason(version: &str, path: &Path, stderr: &[u8]) -> Option<String> {
    let exact_version = version
        .split_whitespace()
        .any(|part| part.trim_start_matches('v') == "0.60.0");
    let package_lock = path
        .file_name()
        .is_some_and(|name| name == "package-lock.json");
    let message = String::from_utf8_lossy(stderr);
    let official_message = message.contains("No parser could be inferred for file");
    (exact_version && package_lock && official_message)
        .then(|| "Oxfmt 0.60.0 does not support package-lock.json via stdin".into())
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::runtime_unsupported_reason;

    #[test]
    fn only_known_oxfmt_failure_is_unsupported() {
        assert!(
            runtime_unsupported_reason(
                "oxfmt 0.60.0",
                Path::new("package-lock.json"),
                b"No parser could be inferred for file package-lock.json",
            )
            .is_some()
        );
        assert!(
            runtime_unsupported_reason(
                "oxfmt 0.60.0",
                Path::new("package-lock.json"),
                b"unexpected unsupported crash",
            )
            .is_none()
        );
    }
}
