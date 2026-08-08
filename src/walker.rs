use std::io::Error as io_Error;
use std::path::PathBuf;
use thiserror::Error;
use walkdir::WalkDir;

use crate::symlink::{Symlink, SymlinkError};

#[derive(Error, Debug)]
pub enum WalkerError {
    #[error("Root of walker does not exist or isn't a directory: {0}")]
    RootIsNotADir(PathBuf),
    #[error("Subroot is not inside root: {0}")]
    SubrootNotInRoot(PathBuf),
    #[error("IO Error: {0}")]
    Io(#[from] io_Error),
    #[error("Symlink Error: {0}")]
    Symlink(#[from] SymlinkError),
}

pub struct Walker {
    root: PathBuf,
    subroot: PathBuf,
}

impl Walker {
    pub fn new(root: PathBuf, subroot: Option<PathBuf>) -> Result<Self, WalkerError> {
        if !root.is_dir() {
            return Err(WalkerError::RootIsNotADir(root));
        }

        let subroot = subroot.unwrap_or_else(|| root.clone());

        if !subroot.starts_with(&root) {
            return Err(WalkerError::SubrootNotInRoot(subroot));
        }

        if !subroot.is_dir() {
            return Err(WalkerError::RootIsNotADir(subroot));
        }

        Ok(Self { root, subroot })
    }

    pub fn root(&self) -> &std::path::Path {
        &self.root
    }

    pub fn subroot(&self) -> &std::path::Path {
        &self.subroot
    }

    pub fn search_symlinks(&self) -> Result<Vec<Symlink>, WalkerError> {
        let mut symlinks = Vec::new();

        for entry in WalkDir::new(&self.subroot) {
            match entry {
                Ok(entry) if entry.path_is_symlink() => {
                    let path = entry.path().to_path_buf();
                    let symlink = Symlink::new(&self.root, path)?;
                    symlinks.push(symlink);
                }
                Ok(_) => {}
                Err(e) => eprintln!("error: {e}"),
            }
        }

        Ok(symlinks)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::{self, File};
    use std::os::unix::fs::symlink;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static DIR_COUNTER: AtomicUsize = AtomicUsize::new(0);

    struct TestDir {
        path: PathBuf,
    }

    impl TestDir {
        fn new() -> Self {
            let n = DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
            let dir = std::env::temp_dir().join(format!(
                "symlinks_db_walker_test_{}_{}",
                std::process::id(),
                n
            ));
            fs::create_dir_all(&dir).unwrap();
            Self { path: dir }
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    #[test]
    fn subroot_must_be_inside_root() {
        let dir = TestDir::new();
        let root = dir.path.join("root");
        fs::create_dir_all(&root).unwrap();

        let outside = dir.path.join("outside");
        fs::create_dir_all(&outside).unwrap();

        assert!(Walker::new(root.clone(), Some(outside)).is_err());
        assert!(Walker::new(root, None).is_ok());
    }

    #[test]
    fn root_must_be_a_directory() {
        let dir = TestDir::new();
        let file = dir.path.join("file");
        File::create(&file).unwrap();

        assert!(Walker::new(file, None).is_err());
    }

    #[test]
    fn finds_symlinks_only_in_subroot() {
        let dir = TestDir::new();
        let root = dir.path.join("root");
        let inside = root.join("inside");
        let outside = root.join("outside");
        fs::create_dir_all(&inside).unwrap();
        fs::create_dir_all(&outside).unwrap();
        let target = inside.join("target");
        File::create(&target).unwrap();

        symlink(&target, inside.join("in_link")).unwrap();
        symlink(&target, outside.join("out_link")).unwrap();

        let walker = Walker::new(root.clone(), Some(inside)).unwrap();
        let found = walker.search_symlinks().unwrap();
        assert_eq!(found.len(), 1);
        assert!(found[0].path().ends_with("in_link"));
    }

    #[test]
    fn skips_symlinks_escaping_root() {
        let dir = TestDir::new();
        let root = dir.path.join("root");
        let outside = dir.path.join("outside");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&outside).unwrap();
        let target = outside.join("target");
        File::create(&target).unwrap();

        let link = root.join("esc");
        symlink(&target, &link).unwrap();

        let walker = Walker::new(root, None).unwrap();
        let err = walker.search_symlinks().unwrap_err();
        assert!(matches!(
            err,
            WalkerError::Symlink(SymlinkError::TargetOutsideRoot { .. })
        ));
    }
}
