//! Fixture host trees for provider tests: a private temporary directory in
//! which a test writes exactly the `/proc`, `/sys` and `/etc` files it
//! wants, and nothing else exists.

use std::path::{Path, PathBuf};

use super::HostRoot;

/// A fixture host tree, removed on drop.
pub(crate) struct Tree {
    dir: PathBuf,
}

impl Tree {
    /// An empty tree.
    pub(crate) fn new() -> Self {
        let mut unique = [0_u8; 8];
        getrandom::fill(&mut unique).expect("random");
        let dir = std::env::temp_dir().join(format!(
            "atrium-host-{}-{}",
            std::process::id(),
            hex::encode(unique)
        ));
        std::fs::create_dir_all(&dir).expect("fixture root");
        Self { dir }
    }

    /// The tree as a provider root.
    pub(crate) fn root(&self) -> HostRoot {
        HostRoot::fixture(&self.dir)
    }

    /// The path of `relative` in the tree.
    pub(crate) fn path(&self, relative: &str) -> PathBuf {
        self.dir.join(relative)
    }

    /// Writes `content` at `relative`, creating directories.
    pub(crate) fn write(&self, relative: &str, content: &str) {
        let path = self.path(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("fixture dir");
        }
        std::fs::write(path, content).expect("fixture file");
    }

    /// Creates a directory at `relative`.
    pub(crate) fn mkdir(&self, relative: &str) {
        std::fs::create_dir_all(self.path(relative)).expect("fixture dir");
    }

    /// A symbolic link at `relative` pointing at `target`.
    pub(crate) fn symlink(&self, target: &Path, relative: &str) {
        let path = self.path(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("fixture dir");
        }
        std::os::unix::fs::symlink(target, path).expect("fixture link");
    }
}

impl Drop for Tree {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}
