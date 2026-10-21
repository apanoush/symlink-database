//! Chain analysis over stored symlinks.
//!
//! The database stores a link's normalized target, not its final canonical
//! destination (see `symlink`). When that target is itself a stored symlink,
//! the link points at another symlink. This module turns the stored rows into a
//! [`ChainReport`] without touching the database or the filesystem.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::database::Record;

/// A symlink whose immediate target is another symlink.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ChainLink {
    /// The reported symlink.
    pub path: PathBuf,
    /// Downstream symlinks after `path`, nearest first. Empty when the link
    /// points directly at a symlink that does not point at another one.
    pub chain: Vec<PathBuf>,
    /// Final non-symlink path, the repeated node of a cycle, or a missing path.
    pub terminal: PathBuf,
    /// Whether the chain revisits a node.
    pub cycle: bool,
    /// Whether the starting symlink cannot resolve.
    pub broken: bool,
}

/// Whole-database chain report.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ChainReport {
    /// Every symlink that points at another symlink, sorted by path.
    pub links: Vec<ChainLink>,
    /// Number of reported links whose chain does not resolve.
    pub broken: usize,
    /// Number of reported links that participate in a cycle.
    pub cycles: usize,
    /// Symlink count of the longest reported chain.
    pub longest: usize,
}

/// Find every stored symlink whose target is another stored symlink.
///
/// `records` is normally [`crate::database::Database::all`]. Rows missing from
/// the database (for example because the target sits in a skipped directory)
/// cannot be followed and are not reported.
pub fn collect(records: &[Record]) -> ChainReport {
    let by_path: HashMap<&Path, &Record> = records
        .iter()
        .map(|record| (record.path.as_path(), record))
        .collect();

    let mut links = Vec::new();
    for record in records {
        if !by_path.contains_key(record.target.as_path()) {
            continue;
        }

        let mut chain = Vec::new();
        let mut seen: HashSet<&Path> = HashSet::from([record.path.as_path()]);
        let mut current = record;
        let (terminal, cycle) = loop {
            match by_path.get(current.target.as_path()) {
                None => break (current.target.clone(), false),
                Some(next) if !seen.insert(next.path.as_path()) => {
                    break (next.path.clone(), true);
                }
                Some(next) => {
                    chain.push(next.path.clone());
                    current = next;
                }
            }
        };

        links.push(ChainLink {
            path: record.path.clone(),
            chain,
            terminal,
            cycle,
            broken: record.broken,
        });
    }

    links.sort_by(|a, b| a.path.cmp(&b.path));
    ChainReport {
        broken: links.iter().filter(|link| link.broken).count(),
        cycles: links.iter().filter(|link| link.cycle).count(),
        longest: links
            .iter()
            .map(|link| link.chain.len() + 1)
            .max()
            .unwrap_or(0),
        links,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(id: i64, path: &str, target: &str, broken: bool) -> Record {
        Record {
            id,
            path: PathBuf::from(path),
            target: PathBuf::from(target),
            broken,
            added_at: 0,
        }
    }

    fn find<'a>(report: &'a ChainReport, path: &str) -> &'a ChainLink {
        report
            .links
            .iter()
            .find(|link| link.path == Path::new(path))
            .unwrap()
    }

    #[test]
    fn link_to_regular_file_is_not_reported() {
        let report = collect(&[record(1, "a", "file.txt", false)]);
        assert!(report.links.is_empty());
        assert_eq!(report.longest, 0);
    }

    #[test]
    fn one_hop_chain_reports_terminal_file() {
        let report = collect(&[
            record(1, "a", "b", false),
            record(2, "b", "file.txt", false),
        ]);

        assert_eq!(report.links.len(), 1);
        let link = find(&report, "a");
        assert_eq!(link.chain, vec![PathBuf::from("b")]);
        assert_eq!(link.terminal, PathBuf::from("file.txt"));
        assert!(!link.cycle);
        assert!(!link.broken);
        assert_eq!(report.longest, 2);
        assert_eq!(report.broken, 0);
        assert_eq!(report.cycles, 0);
    }

    #[test]
    fn multi_hop_chain_collects_every_downstream_symlink() {
        let report = collect(&[
            record(1, "a", "b", false),
            record(2, "b", "c", false),
            record(3, "c", "d", false),
        ]);

        let link = find(&report, "a");
        assert_eq!(link.chain, vec![PathBuf::from("b"), PathBuf::from("c")]);
        assert_eq!(link.terminal, PathBuf::from("d"));
        assert_eq!(report.longest, 3);
    }

    #[test]
    fn self_loop_is_a_cycle() {
        let report = collect(&[record(1, "a", "a", true)]);

        let link = find(&report, "a");
        assert!(link.chain.is_empty());
        assert_eq!(link.terminal, PathBuf::from("a"));
        assert!(link.cycle);
        assert!(link.broken);
        assert_eq!(report.cycles, 1);
    }

    #[test]
    fn two_node_cycle_reports_both_links() {
        let report = collect(&[record(1, "a", "b", true), record(2, "b", "a", true)]);

        assert_eq!(report.links.len(), 2);
        let a = find(&report, "a");
        assert_eq!(a.chain, vec![PathBuf::from("b")]);
        assert_eq!(a.terminal, PathBuf::from("a"));
        assert!(a.cycle);
        let b = find(&report, "b");
        assert_eq!(b.chain, vec![PathBuf::from("a")]);
        assert_eq!(b.terminal, PathBuf::from("b"));
        assert!(b.cycle);
        assert_eq!(report.cycles, 2);
    }

    #[test]
    fn broken_terminal_is_kept() {
        let report = collect(&[
            record(1, "a", "b", true),
            record(2, "b", "missing.txt", true),
        ]);

        let link = find(&report, "a");
        assert_eq!(link.terminal, PathBuf::from("missing.txt"));
        assert!(link.broken);
        assert!(!link.cycle);
        assert_eq!(report.broken, 1);
    }

    #[test]
    fn unrecorded_target_symlink_is_not_followed() {
        let report = collect(&[record(1, "a", "skipped/b", false)]);
        assert!(report.links.is_empty());
    }

    #[test]
    fn report_is_sorted_and_counted() {
        let report = collect(&[
            record(1, "c", "z", false),
            record(2, "a", "z", false),
            record(3, "z", "file.txt", false),
        ]);

        let paths: Vec<&Path> = report
            .links
            .iter()
            .map(|link| link.path.as_path())
            .collect();
        assert_eq!(paths, vec![Path::new("a"), Path::new("c")]);
        assert_eq!(report.longest, 2);
        assert_eq!(report.broken, 0);
        assert_eq!(report.cycles, 0);
    }
}
