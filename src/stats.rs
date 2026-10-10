//! Aggregation over the symlink database.
//!
//! This module is pure SQL: it never touches a terminal and has no `ratatui`
//! dependency. It turns the `symlinks` table into a single [`Stats`] view
//! model that both the interactive TUI and the non-interactive `stats`
//! command render. Keeping the aggregation here means `--json` output, the
//! TUI and the text report all share one source of truth.
//!
//! Stored `path` and `target` columns are relative to the scan root
//! (see `README.md`). A "depth-1 folder" is the first path component, or
//! `(root)` for entries that live directly at the root.

use rusqlite::Connection;
use serde::Serialize;

use crate::database::DatabaseError;

/// A target (usually a file) that receives one or more links.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TargetStat {
    /// Link target, relative to the scan root.
    pub target: String,
    /// Number of symlinks pointing at this target.
    pub links: u64,
    /// How many of those links are broken.
    pub broken: u64,
}

/// A depth-1 folder and a link count.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FolderStat {
    /// First path component, or `(root)`.
    pub folder: String,
    /// Number of symlinks counted for this folder.
    pub links: u64,
}

/// Broken-link health for a single depth-1 folder.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FolderHealth {
    /// First path component, or `(root)`.
    pub folder: String,
    /// Number of symlinks counted for this folder.
    pub links: u64,
    /// How many of those links are broken.
    pub broken: u64,
    /// `broken / links * 100`.
    pub broken_percent: f64,
}

/// How many targets receive exactly `links` links.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LinksBucket {
    /// Links per target.
    pub links: u64,
    /// Number of targets that have exactly `links` incoming links.
    pub targets: u64,
}

/// One symlink row, used by the "broken" and "newest" lists.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LinkStat {
    /// Symlink location, relative to the scan root.
    pub path: String,
    /// Resolved link target, relative to the scan root.
    pub target: String,
    /// Whether the target is missing.
    pub broken: bool,
    /// Unix timestamp of first import.
    pub added_at: i64,
}

/// Whole-database totals.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Totals {
    /// Total number of symlinks stored.
    pub total: u64,
    /// Number of broken symlinks.
    pub broken: u64,
    /// `broken / total * 100`.
    pub broken_percent: f64,
    /// Number of distinct link targets.
    pub unique_targets: u64,
    /// Oldest first-import timestamp, if any.
    pub oldest_added_at: Option<i64>,
    /// Newest first-import timestamp, if any.
    pub newest_added_at: Option<i64>,
}

/// Everything the UI shows, computed once per refresh.
///
/// `Serialize` is derived so `lndb stats --json` and future consumers come for
/// free. Keep formatting (units, dates, padding) out of this struct.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Stats {
    /// Whole-database totals.
    pub totals: Totals,
    /// Files with the most incoming links, highest first.
    pub top_targets: Vec<TargetStat>,
    /// Depth-1 folders by number of symlinks they directly contain,
    /// highest first.
    pub folders: Vec<FolderStat>,
    /// Depth-1 target directories by number of incoming links, highest first.
    pub target_dirs: Vec<FolderStat>,
    /// Broken-link health per depth-1 folder, highest link count first.
    pub folder_health: Vec<FolderHealth>,
    /// Distribution of incoming links per target, ascending.
    pub links_per_target: Vec<LinksBucket>,
    /// Symlinks ordered by first-import time, newest first.
    pub newest: Vec<LinkStat>,
    /// Every broken symlink, ordered by path.
    pub broken: Vec<LinkStat>,
}

/// Compute the whole view model with a set of read-only SQL queries.
pub fn collect(conn: &Connection) -> Result<Stats, DatabaseError> {
    Ok(Stats {
        totals: totals(conn)?,
        top_targets: top_targets(conn)?,
        folders: folders(conn)?,
        target_dirs: target_dirs(conn)?,
        folder_health: folder_health(conn)?,
        links_per_target: links_per_target(conn)?,
        newest: newest(conn)?,
        broken: broken(conn)?,
    })
}

/// Format a Unix timestamp (UTC) as `YYYY-MM-DD HH:MM`.
pub fn format_epoch(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}",
        rem / 3600,
        (rem % 3600) / 60
    )
}

/// Convert days since the Unix epoch to a proleptic Gregorian `(year, month, day)`.
///
/// Howard Hinnant's `civil_from_days` algorithm, which is exact for the whole
/// range of `i64` days.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32; // [1, 12]
    (if m <= 2 { y + 1 } else { y }, m, d)
}

fn percent(part: u64, whole: u64) -> f64 {
    if whole == 0 {
        0.0
    } else {
        part as f64 * 100.0 / whole as f64
    }
}

fn totals(conn: &Connection) -> Result<Totals, DatabaseError> {
    let (total, broken, unique_targets, oldest, newest): (i64, i64, i64, Option<i64>, Option<i64>) =
        conn.query_row(
            "SELECT COUNT(*),
                    COALESCE(SUM(broken), 0),
                    COUNT(DISTINCT target),
                    MIN(added_at),
                    MAX(added_at)
             FROM symlinks",
            [],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            },
        )?;
    let total = total as u64;
    let broken = broken as u64;
    Ok(Totals {
        total,
        broken,
        broken_percent: percent(broken, total),
        unique_targets: unique_targets as u64,
        oldest_added_at: oldest,
        newest_added_at: newest,
    })
}

fn top_targets(conn: &Connection) -> Result<Vec<TargetStat>, DatabaseError> {
    let mut stmt = conn.prepare(
        "SELECT target, COUNT(*) AS links, COALESCE(SUM(broken), 0) AS broken
         FROM symlinks
         GROUP BY target
         ORDER BY links DESC, target ASC",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(TargetStat {
            target: row.get(0)?,
            links: row.get::<_, i64>(1)? as u64,
            broken: row.get::<_, i64>(2)? as u64,
        })
    })?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(DatabaseError::from)
}

fn folders(conn: &Connection) -> Result<Vec<FolderStat>, DatabaseError> {
    // Depth-1 immediate children only: a path with at most one `/` is either a
    // link directly at the root (0 slashes) or inside a depth-1 folder (1 slash).
    let mut stmt = conn.prepare(
        "SELECT CASE WHEN instr(path, '/') = 0 THEN '(root)'
                     ELSE substr(path, 1, instr(path, '/') - 1) END AS folder,
                COUNT(*) AS links
         FROM symlinks
         WHERE (length(path) - length(replace(path, '/', ''))) <= 1
         GROUP BY folder
         ORDER BY links DESC, folder ASC",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(FolderStat {
            folder: row.get(0)?,
            links: row.get::<_, i64>(1)? as u64,
        })
    })?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(DatabaseError::from)
}

fn target_dirs(conn: &Connection) -> Result<Vec<FolderStat>, DatabaseError> {
    // Top-level target folder, counting links at any depth.
    let mut stmt = conn.prepare(
        "SELECT CASE WHEN instr(target, '/') = 0 THEN '(root)'
                     ELSE substr(target, 1, instr(target, '/') - 1) END AS folder,
                COUNT(*) AS links
         FROM symlinks
         GROUP BY folder
         ORDER BY links DESC, folder ASC",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(FolderStat {
            folder: row.get(0)?,
            links: row.get::<_, i64>(1)? as u64,
        })
    })?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(DatabaseError::from)
}

fn folder_health(conn: &Connection) -> Result<Vec<FolderHealth>, DatabaseError> {
    let mut stmt = conn.prepare(
        "SELECT CASE WHEN instr(path, '/') = 0 THEN '(root)'
                     ELSE substr(path, 1, instr(path, '/') - 1) END AS folder,
                COUNT(*) AS links,
                COALESCE(SUM(broken), 0) AS broken
         FROM symlinks
         WHERE (length(path) - length(replace(path, '/', ''))) <= 1
         GROUP BY folder
         ORDER BY links DESC, folder ASC",
    )?;
    let rows = stmt.query_map([], |row| {
        let links = row.get::<_, i64>(1)? as u64;
        let broken = row.get::<_, i64>(2)? as u64;
        Ok(FolderHealth {
            folder: row.get(0)?,
            links,
            broken,
            broken_percent: percent(broken, links),
        })
    })?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(DatabaseError::from)
}

fn links_per_target(conn: &Connection) -> Result<Vec<LinksBucket>, DatabaseError> {
    let mut stmt = conn.prepare(
        "SELECT links, COUNT(*) AS targets
         FROM (SELECT COUNT(*) AS links FROM symlinks GROUP BY target)
         GROUP BY links
         ORDER BY links ASC",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(LinksBucket {
            links: row.get::<_, i64>(0)? as u64,
            targets: row.get::<_, i64>(1)? as u64,
        })
    })?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(DatabaseError::from)
}

fn newest(conn: &Connection) -> Result<Vec<LinkStat>, DatabaseError> {
    let mut stmt = conn.prepare(
        "SELECT path, target, broken, added_at
         FROM symlinks
         ORDER BY added_at DESC, path ASC",
    )?;
    let rows = stmt.query_map([], link_stat_from_row)?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(DatabaseError::from)
}

fn broken(conn: &Connection) -> Result<Vec<LinkStat>, DatabaseError> {
    let mut stmt = conn.prepare(
        "SELECT path, target, broken, added_at
         FROM symlinks
         WHERE broken = 1
         ORDER BY path ASC",
    )?;
    let rows = stmt.query_map([], link_stat_from_row)?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(DatabaseError::from)
}

fn link_stat_from_row(row: &rusqlite::Row) -> rusqlite::Result<LinkStat> {
    Ok(LinkStat {
        path: row.get(0)?,
        target: row.get(1)?,
        broken: row.get::<_, i64>(2)? != 0,
        added_at: row.get(3)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::Database;

    fn db_with(rows: &[(&str, &str, bool, i64)]) -> Database {
        let db = Database::in_memory().unwrap();
        for (path, target, broken, added_at) in rows {
            db.connection()
                .execute(
                    "INSERT INTO symlinks (path, target, broken, added_at)
                     VALUES (?1, ?2, ?3, ?4)",
                    rusqlite::params![path, target, *broken as i64, added_at],
                )
                .unwrap();
        }
        db
    }

    #[test]
    fn totals_counts_links_targets_and_broken() {
        let db = db_with(&[
            ("a/l1", "docs/x", false, 10),
            ("a/l2", "docs/x", false, 20),
            ("b/l3", "docs/y", true, 30),
        ]);
        let s = collect(db.connection()).unwrap();
        assert_eq!(s.totals.total, 3);
        assert_eq!(s.totals.broken, 1);
        assert_eq!(s.totals.unique_targets, 2);
        assert_eq!(s.totals.oldest_added_at, Some(10));
        assert_eq!(s.totals.newest_added_at, Some(30));
        assert!((s.totals.broken_percent - 100.0 / 3.0).abs() < 1e-9);
    }

    #[test]
    fn totals_defaults_are_zero_for_empty_db() {
        let db = db_with(&[]);
        let s = collect(db.connection()).unwrap();
        assert_eq!(s.totals.total, 0);
        assert_eq!(s.totals.broken_percent, 0.0);
        assert_eq!(s.totals.oldest_added_at, None);
    }

    #[test]
    fn top_targets_orders_by_link_count() {
        let db = db_with(&[
            ("a/l1", "docs/x", false, 1),
            ("a/l2", "docs/x", false, 1),
            ("b/l3", "docs/y", true, 1),
        ]);
        let s = collect(db.connection()).unwrap();
        assert_eq!(s.top_targets.len(), 2);
        assert_eq!(s.top_targets[0].target, "docs/x");
        assert_eq!(s.top_targets[0].links, 2);
        assert_eq!(s.top_targets[0].broken, 0);
        assert_eq!(s.top_targets[1].target, "docs/y");
        assert_eq!(s.top_targets[1].broken, 1);
    }

    #[test]
    fn folders_count_immediate_children_only() {
        let db = db_with(&[
            ("a/l1", "docs/x", false, 1),
            ("a/l2", "docs/x", false, 1),
            ("a/deep/l3", "docs/x", false, 1), // depth 2: excluded
            ("b/l4", "docs/y", false, 1),
            ("top", "docs/y", false, 1), // at root
        ]);
        let s = collect(db.connection()).unwrap();
        let by_name = |name: &str| s.folders.iter().find(|f| f.folder == name).map(|f| f.links);
        assert_eq!(by_name("a"), Some(2));
        assert_eq!(by_name("b"), Some(1));
        assert_eq!(by_name("(root)"), Some(1));
        assert_eq!(by_name("a/deep"), None);
    }

    #[test]
    fn target_dirs_group_by_top_level_and_count_all_depths() {
        let db = db_with(&[
            ("a/l1", "docs/deep/x", false, 1),
            ("a/l2", "docs/deep/x", false, 1),
            ("b/l3", "assets/y", false, 1),
            ("root_link", "z", false, 1),
        ]);
        let s = collect(db.connection()).unwrap();
        let by_name = |name: &str| {
            s.target_dirs
                .iter()
                .find(|f| f.folder == name)
                .map(|f| f.links)
        };
        assert_eq!(by_name("docs"), Some(2));
        assert_eq!(by_name("assets"), Some(1));
        assert_eq!(by_name("(root)"), Some(1));
    }

    #[test]
    fn folder_health_reports_broken_percent() {
        let db = db_with(&[
            ("a/l1", "docs/x", true, 1),
            ("a/l2", "docs/x", false, 1),
            ("b/l3", "docs/y", false, 1),
        ]);
        let s = collect(db.connection()).unwrap();
        let a = s.folder_health.iter().find(|f| f.folder == "a").unwrap();
        assert_eq!(a.links, 2);
        assert_eq!(a.broken, 1);
        assert!((a.broken_percent - 50.0).abs() < 1e-9);
    }

    #[test]
    fn links_per_target_distribution() {
        let db = db_with(&[
            ("a/l1", "docs/x", false, 1),
            ("a/l2", "docs/x", false, 1),
            ("b/l3", "docs/y", false, 1),
            ("c/l4", "docs/z", false, 1),
        ]);
        let s = collect(db.connection()).unwrap();
        assert_eq!(
            s.links_per_target,
            vec![
                LinksBucket {
                    links: 1,
                    targets: 2
                },
                LinksBucket {
                    links: 2,
                    targets: 1
                },
            ]
        );
    }

    #[test]
    fn newest_and_broken_lists() {
        let db = db_with(&[
            ("a/l1", "docs/x", false, 10),
            ("b/l2", "docs/y", true, 30),
            ("c/l3", "docs/z", true, 20),
        ]);
        let s = collect(db.connection()).unwrap();
        assert_eq!(s.newest[0].path, "b/l2");
        assert_eq!(s.newest[2].path, "a/l1");
        assert_eq!(
            s.broken.iter().map(|l| l.path.as_str()).collect::<Vec<_>>(),
            vec!["b/l2", "c/l3"]
        );
    }

    #[test]
    fn format_epoch_known_values() {
        assert_eq!(format_epoch(0), "1970-01-01 00:00");
        assert_eq!(format_epoch(1_700_000_000), "2023-11-14 22:13");
    }
}
