use std::collections::HashSet;
use std::env::VarError;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

use clap::{Parser, Subcommand};
use indicatif::{ProgressBar, ProgressStyle};
use thiserror::Error;

use crate::config::Config;
use crate::database::{Database, ImportResult};
use crate::symlink::{Symlink, SymlinkError, normalize};
use crate::walker::{Walker, WalkerError};

const BOLD: &str = "\x1b[1m";
const RESET: &str = "\x1b[0m";

fn bold(n: usize) -> String {
    format!("{BOLD}{n}{RESET}")
}

/// Strip `root` from an absolute `target`; if not under root, keep as-is.
/// Falls back to canonicalizing both when the lexical spelling differs
/// (e.g. `root` reached through a symlink), since DB targets are root-relative.
pub fn relativize(target: &Path, root: &Path) -> PathBuf {
    if let Ok(rel) = target.strip_prefix(root) {
        return normalize(rel);
    }
    if let (Ok(can_root), Ok(can_target)) = (fs::canonicalize(root), fs::canonicalize(target))
        && let Ok(rel) = can_target.strip_prefix(&can_root)
    {
        return normalize(rel);
    }
    normalize(target)
}

/// Make `target` absolute (relative → CWD), then relativize against `root`.
pub fn resolve_target(target: &Path, root: &Path) -> Result<PathBuf, std::io::Error> {
    let target = if target.is_absolute() {
        target.to_path_buf()
    } else {
        std::env::current_dir()?.join(target)
    };
    Ok(relativize(&target, root))
}

#[derive(Parser)]
#[command(
    name = "lndb",
    version,
    about = "Store and analyze symlinks found under a root"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Scan the filesystem root and synchronize the database with it
    Sync {
        /// Only walk this subfolder (must be inside root)
        #[arg(long, value_name = "PATH")]
        subroot: Option<PathBuf>,
    },
    /// Add a single symlink into the database
    Import {
        /// Path to the symlink to import
        path: PathBuf,
    },
    /// Show all symlinks in the database that point to the given target
    Find {
        /// Target path that symlinks point to (absolute, $VAR/~, or relative to the current directory)
        target: PathBuf,
        /// Print absolute paths instead of relative ones
        #[arg(long)]
        absolute: bool,
    },
    /// List all symlink paths in the database (paged with less)
    List {
        /// Only show broken symlinks
        #[arg(long)]
        broken: bool,
        /// Print absolute paths instead of relative ones
        #[arg(long)]
        absolute: bool,
    },
}

#[derive(Error, Debug)]
pub enum CliError {
    #[error("Config error: {0}")]
    Config(#[from] crate::config::ConfigError),
    #[error("Database error: {0}")]
    Database(#[from] crate::database::DatabaseError),
    #[error("Walker error: {0}")]
    Walker(#[from] WalkerError),
    #[error("Symlink error: {0}")]
    Symlink(#[from] SymlinkError),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Environment variable error: {0}")]
    Env(#[from] shellexpand::LookupError<VarError>),
}

impl Cli {
    pub fn run(&self) -> Result<(), CliError> {
        match &self.command {
            Commands::Sync { subroot } => self.sync(subroot.clone()),
            Commands::Import { path } => self.import(path.clone()),
            Commands::Find { target, absolute } => self.find(target.clone(), *absolute),
            Commands::List { broken, absolute } => self.list(*broken, *absolute),
        }
    }

    fn sync(&self, subroot: Option<PathBuf>) -> Result<(), CliError> {
        let config = Config::from_config_file()?;

        let walker = Walker::new(config.paths.root.clone(), subroot.clone())?
            .with_skip(move |rel, is_dir| config.skip.matches(rel, is_dir));

        let scan_pb = ProgressBar::new_spinner();
        scan_pb.set_style(
            ProgressStyle::with_template("{spinner} scanning… {msg}").expect("valid template"),
        );
        let scanned = AtomicUsize::new(0);
        let symlinks = walker.search_symlinks_with(
            || {
                let n = scanned.fetch_add(1, Ordering::Relaxed) + 1;
                scan_pb.set_message(format!("{n} entries"));
                scan_pb.inc(1);
            },
            |err| scan_pb.println(format!("Warning: {err}")),
        )?;
        scan_pb.finish_and_clear();

        let database = Database::new(&config.paths.database)?;

        let import_pb = ProgressBar::new(symlinks.len() as u64);
        import_pb.set_style(
            ProgressStyle::with_template("{bar:40} {pos}/{len} importing").expect("valid template"),
        );
        let summary = database.import_many_with(&symlinks, || import_pb.inc(1))?;
        import_pb.finish_and_clear();

        let found: HashSet<PathBuf> = symlinks.iter().map(|sl| sl.path().to_path_buf()).collect();
        let scope = subroot
            .as_deref()
            .and_then(|s| s.strip_prefix(&config.paths.root).ok());
        let deleted = database.remove_missing(&found, scope)?;

        println!(
            "Imported {} symlinks ({} new, {} updated), {} unchanged, removed {} stale entries",
            bold(summary.inserted + summary.updated),
            bold(summary.inserted),
            bold(summary.updated),
            bold(summary.unchanged),
            bold(deleted)
        );
        Ok(())
    }

    fn import(&self, path: PathBuf) -> Result<(), CliError> {
        let config = Config::from_config_file()?;

        let metadata = fs::symlink_metadata(&path)?;
        if !metadata.file_type().is_symlink() {
            return Err(CliError::Symlink(SymlinkError::NotASymlink(path)));
        }

        let symlink = Symlink::new(&config.paths.root, path)?;
        let database = Database::new(&config.paths.database)?;
        let result = database.import(&symlink)?;

        let status = match result {
            ImportResult::Inserted => "new",
            ImportResult::Updated => "updated",
            ImportResult::Unchanged => "unchanged",
        };
        println!("Imported {} ({status})", symlink.path().display());
        Ok(())
    }

    fn find(&self, target: PathBuf, absolute: bool) -> Result<(), CliError> {
        let config = Config::from_config_file()?;

        let binding = target.to_string_lossy();
        let expanded = shellexpand::env(&binding)?;
        let expanded = shellexpand::tilde(&expanded);
        let target = PathBuf::from(expanded.as_ref());
        let target = resolve_target(&target, &config.paths.root)?;

        let database = Database::new(&config.paths.database)?;
        let records = database.find_by_target(&target)?;

        if records.is_empty() {
            println!("no symlinks found for target {}", target.display());
        } else {
            for rec in records {
                let path = if absolute {
                    config.paths.root.join(&rec.path)
                } else {
                    rec.path
                };
                let broken = if rec.broken { " (broken)" } else { "" };
                println!("{}{}", path.display(), broken);
            }
        }
        Ok(())
    }

    fn list(&self, broken: bool, absolute: bool) -> Result<(), CliError> {
        let config = Config::from_config_file()?;
        let database = Database::new(&config.paths.database)?;
        let records = database.all()?;

        let tmp = std::env::temp_dir().join(format!("lndb_list_{}.txt", std::process::id()));
        {
            let mut file = fs::File::create(&tmp)?;
            for rec in records {
                if broken && !rec.broken {
                    continue;
                }
                let path = if absolute {
                    config.paths.root.join(&rec.path)
                } else {
                    rec.path
                };
                let marker = if !broken && rec.broken {
                    " (broken)"
                } else {
                    ""
                };
                writeln!(file, "{}{}", path.display(), marker)?;
            }
        }

        let status = Command::new("less").arg(&tmp).status()?;
        fs::remove_file(&tmp)?;
        if !status.success() {
            return Err(std::io::Error::other(format!("less exited with status {status}")).into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relativize_in_root_strips_root() {
        let root = Path::new("/data");
        let target = Path::new("/data/docs/books/x.epub");
        assert_eq!(relativize(target, root), PathBuf::from("docs/books/x.epub"));
    }

    #[test]
    fn relativize_outside_root_keeps_absolute() {
        let root = Path::new("/data");
        let target = Path::new("/home/user/x.epub");
        assert_eq!(relativize(target, root), target.to_path_buf());
    }

    #[test]
    fn relativize_normalizes_components() {
        let root = Path::new("/data");
        let target = Path::new("/data/books/../docs/x.epub");
        assert_eq!(relativize(target, root), PathBuf::from("docs/x.epub"));
    }

    #[test]
    fn resolve_target_absolute_relativizes() {
        let root = Path::new("/data");
        let target = PathBuf::from("/data/docs/x.epub");
        assert_eq!(
            resolve_target(&target, root).unwrap(),
            PathBuf::from("docs/x.epub")
        );
    }

    #[test]
    fn relativize_handles_symlinked_root() {
        let dir = std::env::temp_dir().join(format!("lndb_relativize_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let real = dir.join("real");
        fs::create_dir_all(real.join("docs").join("books")).unwrap();
        let file = real.join("docs").join("books").join("x.epub");
        fs::write(&file, "").unwrap();
        let link = dir.join("root");
        std::os::unix::fs::symlink(&real, &link).unwrap();

        let target = fs::canonicalize(&file).unwrap();
        assert_eq!(
            relativize(&target, &link),
            PathBuf::from("docs/books/x.epub")
        );

        let _ = fs::remove_dir_all(&dir);
    }
}
