use std::ffi::OsString;
use std::path::Path;

pub fn arguments(path: &Path) -> Vec<OsString> {
    vec![
        OsString::from("format"),
        OsString::from("--stdin-file-path"),
        path.as_os_str().to_owned(),
    ]
}
