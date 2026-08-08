use std::path::PathBuf;
use thiserror::Error;
use walkdir::WalkDir;
use std::fs;
use std::io::Error as io_Error;

use crate::symlink::{Symlink, SymlinkError};

#[derive(Error, Debug)]
enum WalkerError {
    #[error("Root of walker does not exist or isn't a directory: {0}")]
    RootIsNotADir(PathBuf),
    #[error("IO Error: {0}")]
    Io(#[from] io_Error),
    #[error("Symlink Error: {0}")]
    Symlink(#[from] SymlinkError)
}

struct Walker {
    root: PathBuf
}

impl Walker {

    pub fn new(root: PathBuf) -> Result<Self, WalkerError> {
    
        if ! root.is_dir() {
            return Err (WalkerError::RootIsNotADir(root));
        }

        return Ok ( Self { root } )

    }

    pub fn search_symlinks(self) -> Result<Vec<Symlink>, WalkerError> {

        let mut symlinks = Vec::new();

        for entry in WalkDir::new(self.root).follow_links(true) {
            match entry {
                Ok(entry) if entry.path_is_symlink() => { //file_type().is_symlink() => {
                    //println!("{}", entry.path().display());
                    
                    let path = PathBuf::from(entry.path());
                    let target = fs::read_link(&path)?;
                    
                    let symlink = Symlink::new(target, path)?;
                    
                    symlinks.push(symlink);

                }
                Ok(_) => {}
                Err(e) => eprintln!("error: {e}"),
            }
        }

        return Ok(symlinks);
    }

}
