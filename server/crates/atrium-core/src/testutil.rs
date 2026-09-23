//! Fixtures for the unit tests: a complete, private installation tree in a
//! temporary directory, removed when the fixture is dropped.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use crate::layout::Layout;

/// A relocated installation tree.
pub(crate) struct Fixture {
    root: PathBuf,
    pub(crate) layout: Layout,
}

impl Fixture {
    /// `<tmp>/<unique>/etc/atrium` and `.../var/lib/atrium`, both `0700`, as
    /// the installer leaves them before `init-identity` runs.
    pub(crate) fn new(tag: &str) -> Self {
        let mut unique = [0_u8; 6];
        getrandom::fill(&mut unique).expect("randomness");
        let root = std::env::temp_dir().join(format!(
            "atrium-unit-{tag}-{}-{}",
            std::process::id(),
            hex::encode(unique)
        ));
        let layout = Layout::under(&root).expect("absolute temp path");
        for dir in [layout.etc_dir(), layout.state_dir()] {
            std::fs::create_dir_all(dir).expect("fixture directory");
            chmod(dir, 0o700);
        }
        Self { root, layout }
    }

    /// Every regular file under the root, with its bytes, sorted by path.
    pub(crate) fn snapshot(&self) -> Vec<(PathBuf, Vec<u8>)> {
        let mut files = Vec::new();
        walk(&self.root, &mut files);
        files.sort();
        files
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn walk(dir: &Path, files: &mut Vec<(PathBuf, Vec<u8>)>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            walk(&path, files);
        } else if kind.is_file() {
            let bytes = std::fs::read(&path).unwrap_or_default();
            files.push((path, bytes));
        } else {
            files.push((path, Vec::new()));
        }
    }
}

/// `chmod`, panicking on failure.
pub(crate) fn chmod(path: &Path, mode: u32) {
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).expect("chmod");
}
