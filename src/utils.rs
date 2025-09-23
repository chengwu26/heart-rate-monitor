use std::cell::LazyCell;
use std::env::{current_dir, current_exe};
use std::path::{Path, PathBuf};

const EXE_DIR: LazyCell<PathBuf> = LazyCell::new(|| {
    let mut exe_dir = current_exe().unwrap();
    exe_dir.pop();
    exe_dir
});

/// Find the `file`(must be relative path) and return the absolute filepath
///
/// The search order:
///
/// - Current working directory
/// - Binary directory(not canonicalize)
/// - Binary directory(canonicalize)
pub fn find_file(file: impl AsRef<Path>) -> Option<PathBuf> {
    let file = file.as_ref();
    debug_assert!(file.is_relative());

    // Find file in current working directory
    if file.exists() {
        return Some(current_dir().unwrap().join(file));
    }
    // Find file in executable directory(not canonicalize)
    let f = EXE_DIR.join(file);
    if f.exists() {
        return Some(f);
    }
    // Find file in executable directory(canonicalize)
    let f = EXE_DIR.canonicalize().unwrap().join(file);
    if f.exists() {
        return Some(f);
    }
    None
}
