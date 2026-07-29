use std::ffi::OsString;
use std::path::Path;

pub fn arguments(path: &Path) -> Vec<OsString> {
    vec![
        OsString::from("--stdin-filepath"),
        path.as_os_str().to_owned(),
    ]
}
