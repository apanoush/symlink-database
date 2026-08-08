use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;

use clap::{Parser, Subcommand};
use thiserror::Error;

use crate::config::Config;
use crate::database::Database;
use crate::symlink::{Symlink, SymlinkError};
use crate::walker::{Walker, WalkerError};

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
        /// Target path that symlinks point to (relative to root, or absolute)
        target: PathBuf,
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
}

impl Cli {
    pub fn run(&self) -> Result<(), CliError> {
        match &self.command {
            Commands::Sync { subroot } => self.sync(subroot.clone()),
            Commands::Import { path } => self.import(path.clone()),
            Commands::Find { target, absolute } => self.find(target.clone(), *absolute),
        }
    }

    fn sync(&self, subroot: Option<PathBuf>) -> Result<(), CliError> {
        let config = Config::from_config_file()?;

        let walker = Walker::new(config.paths.root.clone(), subroot)?;
        let symlinks = walker.search_symlinks()?;

        let database = Database::new(&config.paths.database)?;
        database.import_many(&symlinks)?;

        let found: HashSet<PathBuf> = symlinks.iter().map(|sl| sl.path().to_path_buf()).collect();
        let deleted = database.remove_missing(&found)?;

        println!(
            "imported {} symlinks, removed {} stale entries",
            symlinks.len(),
            deleted
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
        database.import(&symlink)?;

        println!("imported {}", symlink.path().display());
        Ok(())
    }

    fn find(&self, target: PathBuf, absolute: bool) -> Result<(), CliError> {
        let config = Config::from_config_file()?;

        let target = target
            .strip_prefix(&config.paths.root)
            .unwrap_or(&target)
            .to_path_buf();

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
}
