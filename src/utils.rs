use std::env::{current_dir, current_exe};
use std::sync::LazyLock;

use camino::{Utf8Path, Utf8PathBuf};

static EXE_DIR: LazyLock<Utf8PathBuf> = LazyLock::new(|| {
    let mut exe_dir = current_exe().unwrap();
    exe_dir.pop();
    Utf8PathBuf::from_path_buf(exe_dir).expect("The program path is not valid UTF-8")
});

/// Find the `file`(must be relative path) and return the absolute filepath
///
/// The search order:
///
/// - Current working directory
/// - Binary directory(not canonicalize)
/// - Binary directory(canonicalize)
pub fn find_file(file: impl AsRef<Utf8Path>) -> Option<Utf8PathBuf> {
    let file = file.as_ref();
    debug_assert!(file.is_relative());

    // Find file in current working directory
    if file.exists() {
        return Some(
            Utf8PathBuf::from_path_buf(current_dir().unwrap().join(file))
                .expect("Current working path is not valid UTF-8"),
        );
    }
    // Find file in executable directory(not canonicalize)
    let f = EXE_DIR.join(file);
    if f.exists() {
        return Some(f);
    }
    // Find file in executable directory(canonicalize)
    let f = EXE_DIR.canonicalize_utf8().unwrap().join(file);
    if f.exists() {
        return Some(f);
    }
    None
}
