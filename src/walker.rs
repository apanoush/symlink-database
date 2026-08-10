use std::fs;
use std::io::Error as io_Error;
use std::path::{Path, PathBuf};
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

pub type SkipPredicate = dyn Fn(&Path, bool) -> bool;

pub struct Walker {
    root: PathBuf,
    subroot: PathBuf,
    skip: Option<Box<SkipPredicate>>,
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

        Ok(Self {
            root,
            subroot,
            skip: None,
        })
    }

    pub fn with_skip(mut self, skip: impl Fn(&Path, bool) -> bool + 'static) -> Self {
        self.skip = Some(Box::new(skip));
        self
    }

    pub fn root(&self) -> &std::path::Path {
        &self.root
    }

    pub fn subroot(&self) -> &std::path::Path {
        &self.subroot
    }

    pub fn search_symlinks(&self) -> Result<Vec<Symlink>, WalkerError> {
        self.search_symlinks_with(|| {}, |_| {})
    }

    pub fn search_symlinks_with(
        &self,
        mut on_entry: impl FnMut(),
        mut on_error: impl FnMut(&str),
    ) -> Result<Vec<Symlink>, WalkerError> {
        let mut symlinks = Vec::new();

        let canonical_root = fs::canonicalize(&self.root)?;

        let walker = WalkDir::new(&self.subroot)
            .into_iter()
            .filter_entry(|entry| {
                if entry.depth() == 0 {
                    return true;
                }
                let rel = entry
                    .path()
                    .strip_prefix(&self.root)
                    .unwrap_or(entry.path());
                match &self.skip {
                    Some(skip) => !skip(rel, entry.file_type().is_dir()),
                    None => true,
                }
            });

        for entry in walker {
            match entry {
                Ok(entry) if entry.depth() > 0 && entry.path_is_symlink() => {
                    let path = entry.path().to_path_buf();
                    match Symlink::new_with_canonical_root(&self.root, &canonical_root, path) {
                        Ok(symlink) => symlinks.push(symlink),
                        Err(e) => on_error(&e.to_string()),
                    }
                }
                Ok(_) => {}
                Err(e) => on_error(&e.to_string()),
            }
            on_entry();
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
        assert_eq!(found[0].path(), PathBuf::from("inside/in_link"));
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
        let found = walker.search_symlinks().unwrap();
        assert!(found.is_empty());
    }

    #[test]
    fn skip_predicate_excludes_matching_entries() {
        let dir = TestDir::new();
        let root = dir.path.join("root");
        let venv = root.join(".venv");
        let modules = root.join("node_modules");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&venv).unwrap();
        fs::create_dir_all(&modules).unwrap();
        let target = root.join("target.txt");
        File::create(&target).unwrap();

        symlink(&target, root.join("good_link")).unwrap();
        symlink(&target, venv.join("venv_link")).unwrap();
        symlink(&target, modules.join("mod_link")).unwrap();

        let walker = Walker::new(root, None).unwrap().with_skip(|rel, is_dir| {
            is_dir && (rel.ends_with(".venv") || rel.ends_with("node_modules"))
        });
        let found = walker.search_symlinks().unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].path(), PathBuf::from("good_link"));
    }

    #[test]
    fn symlinked_root_is_not_recorded() {
        let dir = TestDir::new();
        let real = dir.path.join("real");
        let root = dir.path.join("root");
        fs::create_dir_all(&real).unwrap();
        std::os::unix::fs::symlink(&real, &root).unwrap();

        let target = real.join("t.txt");
        File::create(&target).unwrap();
        symlink(&target, real.join("l")).unwrap();

        let walker = Walker::new(root, None).unwrap();
        let found = walker.search_symlinks().unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].path(), PathBuf::from("l"));
    }
}
