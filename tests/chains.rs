use std::fs::{self, File};
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use symlink_database::chain;
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
            "symlinks_db_chain_test_{}_{}",
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

fn root_with_chains(root: &Path) {
    fs::create_dir_all(root).unwrap();
    let target = root.join("target.txt");
    File::create(&target).unwrap();

    // a -> b -> target.txt
    symlink(&target, root.join("b")).unwrap();
    symlink(root.join("b"), root.join("a")).unwrap();
    // plain -> target.txt (not a chain)
    symlink(&target, root.join("plain")).unwrap();
    // self -> self (cycle)
    symlink(root.join("self"), root.join("self")).unwrap();
}

#[test]
fn report_finds_links_to_symlinks() {
    let dir = TestDir::new();
    let root = dir.path.join("root");
    root_with_chains(&root);

    let db = Database::new(&dir.path.join("test.db")).unwrap();
    let found = Walker::new(root, None).unwrap().search_symlinks().unwrap();
    db.import_many(&found).unwrap();

    let report = chain::collect(&db.all().unwrap());
    assert_eq!(report.links.len(), 2);

    let a = report
        .links
        .iter()
        .find(|link| link.path == Path::new("a"))
        .unwrap();
    assert_eq!(a.chain, vec![PathBuf::from("b")]);
    assert_eq!(a.terminal, PathBuf::from("target.txt"));
    assert!(!a.cycle);
    assert!(!a.broken);

    let self_link = report
        .links
        .iter()
        .find(|link| link.path == Path::new("self"))
        .unwrap();
    assert!(self_link.chain.is_empty());
    assert_eq!(self_link.terminal, PathBuf::from("self"));
    assert!(self_link.cycle);
    assert!(self_link.broken);
    assert_eq!(report.cycles, 1);
    assert_eq!(report.longest, 2);
}
