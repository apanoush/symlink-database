use std::fs;
use std::path::{Path, PathBuf};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum SymlinkError {
    #[error("Path do not exist: {0}")]
    DoNotExist(PathBuf),
    #[error("Path is not a symlink: {0}")]
    NotASymlink(PathBuf),
    #[error("Target of symlink {path} resolves outside root {root}: {target}")]
    TargetOutsideRoot {
        path: PathBuf,
        target: PathBuf,
        root: PathBuf,
    },
    #[error("Symlink path {path} is outside root {root}")]
    PathOutsideRoot { path: PathBuf, root: PathBuf },
}

#[derive(Debug)]
pub struct Symlink {
    root: PathBuf,
    target: PathBuf,
    path: PathBuf,
    broken: bool,
}

impl Symlink {
    pub fn new(root: &Path, path: PathBuf) -> Result<Self, SymlinkError> {
        let metadata =
            fs::symlink_metadata(&path).map_err(|_| SymlinkError::DoNotExist(path.clone()))?;

        if !metadata.file_type().is_symlink() {
            return Err(SymlinkError::NotASymlink(path));
        }

        let target = fs::read_link(&path).map_err(|_| SymlinkError::DoNotExist(path.clone()))?;

        let resolved = if target.is_absolute() {
            target.clone()
        } else {
            path.parent()
                .map_or_else(|| target.clone(), |parent| parent.join(&target))
        };

        let canonical_root =
            fs::canonicalize(root).map_err(|_| SymlinkError::DoNotExist(root.to_path_buf()))?;
        let canonical_target = fs::canonicalize(&resolved).unwrap_or_else(|_| resolved.clone());

        if !canonical_target.starts_with(&canonical_root) {
            return Err(SymlinkError::TargetOutsideRoot {
                path,
                target,
                root: root.to_path_buf(),
            });
        }

        let broken = !resolved.exists();

        let rel_path = path
            .strip_prefix(root)
            .map_err(|_| SymlinkError::PathOutsideRoot {
                path: path.clone(),
                root: root.to_path_buf(),
            })?
            .to_path_buf();
        let rel_target = resolved
            .strip_prefix(root)
            .map_err(|_| SymlinkError::TargetOutsideRoot {
                path: path.clone(),
                target,
                root: root.to_path_buf(),
            })?
            .to_path_buf();

        Ok(Self {
            root: root.to_path_buf(),
            target: rel_target,
            path: rel_path,
            broken,
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn target(&self) -> &Path {
        &self.target
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn broken(&self) -> bool {
        self.broken
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
            let dir =
                std::env::temp_dir().join(format!("symlinks_db_test_{}_{}", std::process::id(), n));
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
    fn valid_symlink_is_not_broken() {
        let dir = TestDir::new();
        let root = dir.path.clone();
        let target = root.join("target");
        let link = root.join("link");
        File::create(&target).unwrap();
        symlink(&target, &link).unwrap();

        let sl = Symlink::new(&root, link.clone()).unwrap();
        assert!(!sl.broken());
        assert_eq!(sl.path(), Path::new("link"));
        assert_eq!(sl.target(), Path::new("target"));
    }

    #[test]
    fn broken_symlink_is_broken() {
        let dir = TestDir::new();
        let root = dir.path.clone();
        let link = root.join("broken_link");
        symlink(root.join("missing"), &link).unwrap();

        let sl = Symlink::new(&root, link.clone()).unwrap();
        assert!(sl.broken());
        assert_eq!(sl.path(), Path::new("broken_link"));
        assert_eq!(sl.target(), Path::new("missing"));
    }

    #[test]
    fn relative_target_is_resolved_against_symlink_parent() {
        let dir = TestDir::new();
        let root = dir.path.clone();
        let sub = root.join("sub");
        fs::create_dir_all(&sub).unwrap();
        let target = sub.join("target");
        File::create(&target).unwrap();
        let link = root.join("link");
        symlink(Path::new("sub/target"), &link).unwrap();

        let sl = Symlink::new(&root, link.clone()).unwrap();
        assert!(!sl.broken());
        assert_eq!(sl.path(), Path::new("link"));
        assert_eq!(sl.target(), Path::new("sub/target"));
    }

    #[test]
    fn symlink_escaping_root_is_rejected() {
        let dir = TestDir::new();
        let root = dir.path.join("root");
        let outside = dir.path.join("outside");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&outside).unwrap();
        let target = outside.join("target");
        File::create(&target).unwrap();
        let link = root.join("link");
        symlink(&target, &link).unwrap();

        let err = Symlink::new(&root, link).unwrap_err();
        assert!(matches!(err, SymlinkError::TargetOutsideRoot { .. }));
    }

    #[test]
    fn non_symlink_is_rejected() {
        let dir = TestDir::new();
        let root = dir.path.clone();
        let file = root.join("plain");
        File::create(&file).unwrap();

        let err = Symlink::new(&root, file).unwrap_err();
        assert!(matches!(err, SymlinkError::NotASymlink(_)));
    }

    #[test]
    fn symlink_outside_root_is_rejected() {
        let dir = TestDir::new();
        let root = dir.path.join("root");
        let outside = dir.path.join("outside");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&outside).unwrap();
        let target = root.join("target");
        File::create(&target).unwrap();
        let link = outside.join("link");
        symlink(&target, &link).unwrap();

        let err = Symlink::new(&root, link).unwrap_err();
        assert!(matches!(err, SymlinkError::PathOutsideRoot { .. }));
    }
}
