use std::collections::HashSet;
use std::fs::{self, File};
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use symlink_database::database::Database;
use symlink_database::walker::Walker;

static DIR_COUNTER: AtomicUsize = AtomicUsize::new(0);

struct TestDir {
    path: PathBuf,
}

impl TestDir {
    fn new() -> Self {
        let n = DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "symlinks_db_broken_test_{}_{}",
            std::process::id(),
            n
        ));
        fs::create_dir_all(&path).unwrap();
        Self { path }
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn root_with_mixed_links(root: &Path) {
    fs::create_dir_all(root).unwrap();
    let target = root.join("target.txt");
    File::create(&target).unwrap();
    symlink(&target, root.join("good_link")).unwrap();
    symlink(root.join("missing.txt"), root.join("broken_link")).unwrap();
}

#[test]
fn walker_finds_broken_symlinks_within_root() {
    let dir = TestDir::new();
    let root = dir.path.join("root");
    root_with_mixed_links(&root);

    let found = Walker::new(root, None).unwrap().search_symlinks().unwrap();
    assert_eq!(found.len(), 2);

    let broken = found
        .iter()
        .find(|s| s.path() == Path::new("broken_link"))
        .unwrap();
    assert!(broken.broken());

    let good = found
        .iter()
        .find(|s| s.path() == Path::new("good_link"))
        .unwrap();
    assert!(!good.broken());
}

#[test]
fn sync_keeps_broken_symlinks() {
    let dir = TestDir::new();
    let root = dir.path.join("root");
    root_with_mixed_links(&root);

    let db = Database::new(&dir.path.join("test.db")).unwrap();

    let found = Walker::new(root, None).unwrap().search_symlinks().unwrap();
    db.import_many(&found).unwrap();

    let found_paths: HashSet<PathBuf> = found.iter().map(|s| s.path().to_path_buf()).collect();
    let deleted = db.remove_missing(&found_paths, None).unwrap();
    assert_eq!(deleted, 0);

    let broken = db.find_by_path(Path::new("broken_link")).unwrap().unwrap();
    assert!(broken.broken);
    assert_eq!(db.count().unwrap(), 2);
}
