//! Aggregation over the symlink database.
//!
//! This module is pure SQL: it never touches a terminal and has no `ratatui`
//! dependency. It turns the `symlinks` table into a single [`Stats`] view
//! model that both the interactive TUI and the non-interactive `stats`
//! command render. Keeping the aggregation here means `--json` output, the
//! TUI and the text report all share one source of truth.
//!
//! Stored `path` and `target` columns are relative to the scan root
//! (see `README.md`). The folder views group each entry by the first `depth`
//! components of its containing directory (see [`collect`]); entries whose
//! parent is the root itself are grouped under `(root)`.

use std::collections::HashMap;

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

/// A folder group and a link count.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FolderStat {
    /// First `depth` components of the containing directory, or `(root)`.
    pub folder: String,
    /// Number of symlinks counted for this folder.
    pub links: u64,
}

/// Broken-link health for a single folder group.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FolderHealth {
    /// First `depth` components of the containing directory, or `(root)`.
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

/// Default folder grouping depth used by the CLI and the TUI.
pub const DEFAULT_DEPTH: usize = 2;

/// Everything the UI shows, computed once per refresh.
///
/// `Serialize` is derived so `lndb stats --json` and future consumers come for
/// free. Keep formatting (units, dates, padding) out of this struct.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Stats {
    /// Folder grouping depth used for the `folders`, `target_dirs` and
    /// `folder_health` views.
    pub depth: usize,
    /// Whole-database totals.
    pub totals: Totals,
    /// Files with the most incoming links, highest first.
    pub top_targets: Vec<TargetStat>,
    /// Folders by number of symlinks they hold (first `depth` components of
    /// each symlink's parent), highest first.
    pub folders: Vec<FolderStat>,
    /// Folders by number of incoming links (first `depth` components of each
    /// target's parent), highest first.
    pub target_dirs: Vec<FolderStat>,
    /// Broken-link health per folder group, highest link count first.
    pub folder_health: Vec<FolderHealth>,
    /// Distribution of incoming links per target, ascending.
    pub links_per_target: Vec<LinksBucket>,
    /// Symlinks ordered by first-import time, newest first.
    pub newest: Vec<LinkStat>,
    /// Every broken symlink, ordered by path.
    pub broken: Vec<LinkStat>,
}

impl Default for Stats {
    fn default() -> Self {
        Self {
            depth: DEFAULT_DEPTH,
            totals: Totals::default(),
            top_targets: Vec::new(),
            folders: Vec::new(),
            target_dirs: Vec::new(),
            folder_health: Vec::new(),
            links_per_target: Vec::new(),
            newest: Vec::new(),
            broken: Vec::new(),
        }
    }
}

/// Compute the whole view model.
///
/// `depth` controls how many path components the folder views group by
/// (clamped to at least 1). It is applied to each item's containing directory,
/// so `depth = 1` yields the top-level folders and `depth = 2` their
/// subfolders.
pub fn collect(conn: &Connection, depth: usize) -> Result<Stats, DatabaseError> {
    let depth = depth.max(1);
    let rows = depth_rows(conn)?;
    Ok(Stats {
        depth,
        totals: totals(conn)?,
        top_targets: top_targets(conn)?,
        folders: group_folders(&rows, depth),
        target_dirs: group_target_dirs(&rows, depth),
        folder_health: group_health(&rows, depth),
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

/// The `(path, target, broken)` rows needed for depth grouping, read once.
fn depth_rows(conn: &Connection) -> Result<Vec<(String, String, bool)>, DatabaseError> {
    let mut stmt = conn.prepare("SELECT path, target, broken FROM symlinks")?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, i64>(2)? != 0,
        ))
    })?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(DatabaseError::from)
}

/// The first `depth` components of an item's containing directory.
///
/// The item's own file name is dropped, so `a/b/link` at depth 2 groups under
/// `a/b`; an item directly at the root groups under `(root)`.
fn folder_key(item: &str, depth: usize) -> String {
    let parent = match item.rfind('/') {
        Some(index) => &item[..index],
        None => "",
    };
    let mut key = String::new();
    for component in parent
        .split('/')
        .filter(|part| !part.is_empty())
        .take(depth)
    {
        if !key.is_empty() {
            key.push('/');
        }
        key.push_str(component);
    }
    if key.is_empty() {
        "(root)".to_string()
    } else {
        key
    }
}

fn group_folders(rows: &[(String, String, bool)], depth: usize) -> Vec<FolderStat> {
    let mut map: HashMap<String, u64> = HashMap::new();
    for (path, _, _) in rows {
        *map.entry(folder_key(path, depth)).or_default() += 1;
    }
    let mut out: Vec<FolderStat> = map
        .into_iter()
        .map(|(folder, links)| FolderStat { folder, links })
        .collect();
    out.sort_by(|a, b| b.links.cmp(&a.links).then_with(|| a.folder.cmp(&b.folder)));
    out
}

fn group_target_dirs(rows: &[(String, String, bool)], depth: usize) -> Vec<FolderStat> {
    let mut map: HashMap<String, u64> = HashMap::new();
    for (_, target, _) in rows {
        *map.entry(folder_key(target, depth)).or_default() += 1;
    }
    let mut out: Vec<FolderStat> = map
        .into_iter()
        .map(|(folder, links)| FolderStat { folder, links })
        .collect();
    out.sort_by(|a, b| b.links.cmp(&a.links).then_with(|| a.folder.cmp(&b.folder)));
    out
}

fn group_health(rows: &[(String, String, bool)], depth: usize) -> Vec<FolderHealth> {
    let mut map: HashMap<String, (u64, u64)> = HashMap::new();
    for (path, _, broken) in rows {
        let entry = map.entry(folder_key(path, depth)).or_default();
        entry.0 += 1;
        if *broken {
            entry.1 += 1;
        }
    }
    let mut out: Vec<FolderHealth> = map
        .into_iter()
        .map(|(folder, (links, broken))| FolderHealth {
            folder,
            links,
            broken,
            broken_percent: percent(broken, links),
        })
        .collect();
    out.sort_by(|a, b| b.links.cmp(&a.links).then_with(|| a.folder.cmp(&b.folder)));
    out
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
        let s = collect(db.connection(), DEFAULT_DEPTH).unwrap();
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
        let s = collect(db.connection(), DEFAULT_DEPTH).unwrap();
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
        let s = collect(db.connection(), DEFAULT_DEPTH).unwrap();
        assert_eq!(s.top_targets.len(), 2);
        assert_eq!(s.top_targets[0].target, "docs/x");
        assert_eq!(s.top_targets[0].links, 2);
        assert_eq!(s.top_targets[0].broken, 0);
        assert_eq!(s.top_targets[1].target, "docs/y");
        assert_eq!(s.top_targets[1].broken, 1);
    }

    #[test]
    fn folders_group_by_parent_prefix() {
        let db = db_with(&[
            ("a/l1", "docs/x", false, 1),
            ("a/l2", "docs/x", false, 1),
            ("a/deep/l3", "docs/x", false, 1),
            ("b/l4", "docs/y", false, 1),
            ("top", "docs/y", false, 1), // directly at the root
        ]);

        // depth 1: every symlink under `a/...` counts toward `a`.
        let depth1 = collect(db.connection(), 1).unwrap();
        assert_eq!(depth1.depth, 1);
        let by_name =
            |s: &Stats, name: &str| s.folders.iter().find(|f| f.folder == name).map(|f| f.links);
        assert_eq!(by_name(&depth1, "a"), Some(3));
        assert_eq!(by_name(&depth1, "b"), Some(1));
        assert_eq!(by_name(&depth1, "(root)"), Some(1));

        // depth 2: `a/deep/l3` splits off from `a`.
        let depth2 = collect(db.connection(), 2).unwrap();
        assert_eq!(depth2.depth, 2);
        assert_eq!(by_name(&depth2, "a"), Some(2));
        assert_eq!(by_name(&depth2, "a/deep"), Some(1));
        assert_eq!(by_name(&depth2, "b"), Some(1));
        assert_eq!(by_name(&depth2, "(root)"), Some(1));
    }

    #[test]
    fn target_dirs_group_by_parent_prefix() {
        let db = db_with(&[
            ("a/l1", "docs/deep/x", false, 1),
            ("a/l2", "docs/deep/x", false, 1),
            ("b/l3", "assets/y", false, 1),
            ("root_link", "z", false, 1),
        ]);

        let depth1 = collect(db.connection(), 1).unwrap();
        let by_name = |s: &Stats, name: &str| {
            s.target_dirs
                .iter()
                .find(|f| f.folder == name)
                .map(|f| f.links)
        };
        assert_eq!(by_name(&depth1, "docs"), Some(2));
        assert_eq!(by_name(&depth1, "assets"), Some(1));
        assert_eq!(by_name(&depth1, "(root)"), Some(1));

        let depth2 = collect(db.connection(), 2).unwrap();
        assert_eq!(by_name(&depth2, "docs/deep"), Some(2));
        assert_eq!(by_name(&depth2, "assets"), Some(1));
    }

    #[test]
    fn folder_health_reports_broken_percent_at_depth() {
        let db = db_with(&[
            ("a/l1", "docs/x", true, 1),
            ("a/l2", "docs/x", false, 1),
            ("a/sub/l3", "docs/y", true, 1),
            ("b/l4", "docs/y", false, 1),
        ]);
        let depth1 = collect(db.connection(), 1).unwrap();
        let a = depth1
            .folder_health
            .iter()
            .find(|f| f.folder == "a")
            .unwrap();
        assert_eq!(a.links, 3);
        assert_eq!(a.broken, 2);
        assert!((a.broken_percent - 200.0 / 3.0).abs() < 1e-9);

        let depth2 = collect(db.connection(), 2).unwrap();
        let a_sub = depth2
            .folder_health
            .iter()
            .find(|f| f.folder == "a/sub")
            .unwrap();
        assert_eq!(a_sub.links, 1);
        assert_eq!(a_sub.broken, 1);
        assert!((a_sub.broken_percent - 100.0).abs() < 1e-9);
    }

    #[test]
    fn links_per_target_distribution() {
        let db = db_with(&[
            ("a/l1", "docs/x", false, 1),
            ("a/l2", "docs/x", false, 1),
            ("b/l3", "docs/y", false, 1),
            ("c/l4", "docs/z", false, 1),
        ]);
        let s = collect(db.connection(), DEFAULT_DEPTH).unwrap();
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
        let s = collect(db.connection(), DEFAULT_DEPTH).unwrap();
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
