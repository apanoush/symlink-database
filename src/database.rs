use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{Connection, params};
use thiserror::Error;

use crate::symlink::Symlink;

#[derive(Error, Debug)]
pub enum DatabaseError {
    #[error("Failed to create parent directory for database: {0}")]
    CreateParentDir(#[from] std::io::Error),
    #[error("SQLite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
}

pub struct Database {
    conn: Connection,
}

pub struct Record {
    pub id: i64,
    pub path: PathBuf,
    pub target: PathBuf,
    pub broken: bool,
    pub added_at: i64,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ImportResult {
    Inserted,
    Updated,
    Unchanged,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct ImportSummary {
    pub inserted: usize,
    pub updated: usize,
    pub unchanged: usize,
}

impl Database {
    pub fn new(path: &Path) -> Result<Self, DatabaseError> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        let conn = Connection::open(path)?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS symlinks (
                id       INTEGER PRIMARY KEY AUTOINCREMENT,
                path     TEXT UNIQUE NOT NULL,
                target   TEXT NOT NULL,
                broken   INTEGER NOT NULL,
                added_at INTEGER NOT NULL
            );",
        )?;

        Ok(Self { conn })
    }

    pub fn import(&self, symlink: &Symlink) -> Result<ImportResult, DatabaseError> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        let path = symlink.path().to_string_lossy();
        let target = symlink.target().to_string_lossy();
        let broken = symlink.broken() as i64;

        let previous = self.find_by_path(symlink.path())?;
        let previous_unchanged = previous
            .as_ref()
            .is_some_and(|rec| rec.target == *symlink.target() && rec.broken == symlink.broken());

        self.conn.execute(
            "INSERT INTO symlinks (path, target, broken, added_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(path) DO UPDATE SET
                 target = excluded.target,
                 broken = excluded.broken",
            params![path, target, broken, now],
        )?;

        Ok(match previous {
            None => ImportResult::Inserted,
            Some(_) if previous_unchanged => ImportResult::Unchanged,
            Some(_) => ImportResult::Updated,
        })
    }

    pub fn import_many(&self, symlinks: &[Symlink]) -> Result<ImportSummary, DatabaseError> {
        self.import_many_with(symlinks, || {})
    }

    pub fn import_many_with(
        &self,
        symlinks: &[Symlink],
        mut on_import: impl FnMut(),
    ) -> Result<ImportSummary, DatabaseError> {
        let mut summary = ImportSummary::default();

        let existing: HashSet<String> = self
            .conn
            .prepare("SELECT path FROM symlinks")?
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<_>>()?;

        let tx = self.conn.unchecked_transaction()?;
        let mut stmt = tx.prepare(
            "INSERT INTO symlinks (path, target, broken, added_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(path) DO UPDATE SET
                 target = excluded.target,
                 broken = excluded.broken
             WHERE target IS NOT excluded.target OR broken IS NOT excluded.broken",
        )?;
        for symlink in symlinks {
            let path = symlink.path().to_string_lossy();
            let target = symlink.target().to_string_lossy();
            let broken = symlink.broken() as i64;
            let changed = stmt.execute(params![
                path,
                target,
                broken,
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs() as i64
            ])?;
            if existing.contains(path.as_ref()) {
                if changed > 0 {
                    summary.updated += 1;
                } else {
                    summary.unchanged += 1;
                }
            } else {
                summary.inserted += 1;
            }
            on_import();
        }
        drop(stmt);
        tx.commit()?;
        Ok(summary)
    }

    pub fn remove_missing(
        &self,
        found: &HashSet<PathBuf>,
        scope: Option<&Path>,
    ) -> Result<usize, DatabaseError> {
        let found_str: HashSet<String> = found
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        let all = self.all()?;
        let stale: Vec<String> = all
            .iter()
            .filter(|rec| {
                let in_scope = scope.is_none_or(|s| rec.path.starts_with(s));
                in_scope && !found_str.contains(&rec.path.to_string_lossy().into_owned())
            })
            .map(|rec| rec.path.to_string_lossy().into_owned())
            .collect();

        if stale.is_empty() {
            return Ok(0);
        }

        let tx = self.conn.unchecked_transaction()?;
        for chunk in stale.chunks(500) {
            let placeholders = vec!["?"; chunk.len()].join(",");
            let sql = format!("DELETE FROM symlinks WHERE path IN ({placeholders})");
            tx.execute(&sql, rusqlite::params_from_iter(chunk))?;
        }
        tx.commit()?;
        Ok(stale.len())
    }

    pub fn find_by_target(&self, target: &Path) -> Result<Vec<Record>, DatabaseError> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, path, target, broken, added_at FROM symlinks WHERE target = ?1")?;
        let rows = stmt.query_map(params![target.to_string_lossy()], Self::record_from_row)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(DatabaseError::from)
    }

    pub fn find_by_path(&self, path: &Path) -> Result<Option<Record>, DatabaseError> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, path, target, broken, added_at FROM symlinks WHERE path = ?1")?;
        let mut rows = stmt.query_map(params![path.to_string_lossy()], Self::record_from_row)?;
        rows.next().transpose().map_err(DatabaseError::from)
    }

    pub fn all(&self) -> Result<Vec<Record>, DatabaseError> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, path, target, broken, added_at FROM symlinks")?;
        let rows = stmt.query_map([], Self::record_from_row)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(DatabaseError::from)
    }

    pub fn count(&self) -> Result<usize, DatabaseError> {
        let n: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM symlinks", [], |r| r.get(0))?;
        Ok(n as usize)
    }

    fn record_from_row(row: &rusqlite::Row) -> rusqlite::Result<Record> {
        Ok(Record {
            id: row.get(0)?,
            path: PathBuf::from(row.get::<_, String>(1)?),
            target: PathBuf::from(row.get::<_, String>(2)?),
            broken: row.get::<_, i64>(3)? != 0,
            added_at: row.get(4)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::{self, File};
    use std::os::unix::fs::symlink;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static DIR_COUNTER: AtomicUsize = AtomicUsize::new(0);

    struct TestDb {
        _dir: PathBuf,
        db: Database,
    }

    impl TestDb {
        fn new() -> Self {
            let n = DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
            let dir = std::env::temp_dir().join(format!(
                "symlinks_db_db_test_{}_{}",
                std::process::id(),
                n
            ));
            fs::create_dir_all(&dir).unwrap();
            let db_path = dir.join("test.db");
            let db = Database::new(&db_path).unwrap();
            Self { _dir: dir, db }
        }

        fn symlink(&self, name: &str, broken: bool) -> Symlink {
            let root = self._dir.join("root");
            fs::create_dir_all(&root).unwrap();
            let target = root.join(format!("{name}_target"));
            if !broken {
                File::create(&target).unwrap();
            }
            let link = root.join(name);
            symlink(&target, &link).unwrap();
            Symlink::new(&root, link).unwrap()
        }
    }

    #[test]
    fn import_inserts_new_item() {
        let t = TestDb::new();
        let sl = t.symlink("a", false);
        assert_eq!(t.db.import(&sl).unwrap(), ImportResult::Inserted);
        assert_eq!(t.db.count().unwrap(), 1);
        let rec = t.db.find_by_path(sl.path()).unwrap().unwrap();
        assert_eq!(rec.path, sl.path());
        assert_eq!(rec.target, sl.target());
        assert!(!rec.broken);
        assert!(rec.added_at > 0);
    }

    #[test]
    fn import_many_reports_insert_updated_unchanged() {
        let t = TestDb::new();
        t.symlink("a", false);
        t.symlink("b", false);
        let root = t._dir.join("root");

        let first =
            t.db.import_many(&[
                Symlink::new(&root, root.join("a")).unwrap(),
                Symlink::new(&root, root.join("b")).unwrap(),
            ])
            .unwrap();
        assert_eq!(
            first,
            ImportSummary {
                inserted: 2,
                updated: 0,
                unchanged: 0
            }
        );

        let second =
            t.db.import_many(&[
                Symlink::new(&root, root.join("a")).unwrap(),
                Symlink::new(&root, root.join("b")).unwrap(),
            ])
            .unwrap();
        assert_eq!(
            second,
            ImportSummary {
                inserted: 0,
                updated: 0,
                unchanged: 2
            }
        );

        fs::remove_file(root.join("a_target")).unwrap();
        let third =
            t.db.import_many(&[
                Symlink::new(&root, root.join("a")).unwrap(),
                Symlink::new(&root, root.join("b")).unwrap(),
            ])
            .unwrap();
        assert_eq!(
            third,
            ImportSummary {
                inserted: 0,
                updated: 1,
                unchanged: 1
            }
        );
    }

    #[test]
    fn import_skips_unchanged_item() {
        let t = TestDb::new();
        let sl = t.symlink("a", false);
        t.db.import(&sl).unwrap();
        assert_eq!(t.db.import(&sl).unwrap(), ImportResult::Unchanged);
        assert_eq!(t.db.count().unwrap(), 1);
    }

    #[test]
    fn import_updates_broken_status() {
        let t = TestDb::new();
        let mut sl = t.symlink("a", false);
        t.db.import(&sl).unwrap();
        assert!(!t.db.find_by_path(sl.path()).unwrap().unwrap().broken);

        fs::remove_file(t._dir.join("root").join("a_target")).unwrap();
        let root = t._dir.join("root");
        sl = Symlink::new(&root, root.join(sl.path())).unwrap();
        assert_eq!(t.db.import(&sl).unwrap(), ImportResult::Updated);
        assert!(t.db.find_by_path(sl.path()).unwrap().unwrap().broken);
        assert_eq!(t.db.count().unwrap(), 1);
    }

    #[test]
    fn remove_missing_deletes_absent_items() {
        let t = TestDb::new();
        let sl1 = t.symlink("a", false);
        let sl2 = t.symlink("b", false);
        t.db.import(&sl1).unwrap();
        t.db.import(&sl2).unwrap();

        let mut found = HashSet::new();
        found.insert(sl1.path().to_path_buf());
        let deleted = t.db.remove_missing(&found, None).unwrap();
        assert_eq!(deleted, 1);
        assert!(t.db.find_by_path(sl2.path()).unwrap().is_none());
        assert!(t.db.find_by_path(sl1.path()).unwrap().is_some());
    }

    #[test]
    fn remove_missing_respects_scope() {
        let t = TestDb::new();
        let sl1 = t.symlink("a", false);
        let sl2 = t.symlink("b", false);
        t.db.import(&sl1).unwrap();
        t.db.import(&sl2).unwrap();

        let found = HashSet::new();
        let deleted = t.db.remove_missing(&found, Some(Path::new("a"))).unwrap();
        assert_eq!(deleted, 1);
        assert!(t.db.find_by_path(sl1.path()).unwrap().is_none());
        assert!(t.db.find_by_path(sl2.path()).unwrap().is_some());
    }

    #[test]
    fn find_by_target_returns_matching_items() {
        let t = TestDb::new();
        let sl = t.symlink("a", false);
        t.db.import(&sl).unwrap();

        let matches = t.db.find_by_target(sl.target()).unwrap();
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].path, sl.path());
        assert!(
            t.db.find_by_target(Path::new("/nonexistent"))
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn creates_database_and_parent_directory() {
        let dir =
            std::env::temp_dir().join(format!("symlinks_db_db_create_test_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let db_path = dir.join("nested").join("test.db");
        let db = Database::new(&db_path).unwrap();
        assert_eq!(db.count().unwrap(), 0);
        let _ = fs::remove_dir_all(&dir);
    }
}
