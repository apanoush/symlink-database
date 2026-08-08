use std::path::PathBuf;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum SymlinkError {
    #[error("Symlink do not exist: {0}")]
    DoNotExist(PathBuf)
}

pub struct Symlink {
    target: PathBuf,
    path: PathBuf,
    broken: bool
}

impl Symlink {
    
    pub fn new(target: PathBuf, path: PathBuf) -> Result<Self, SymlinkError> {
        
        if ! path.exists() {
            return Err(SymlinkError::DoNotExist(path));
        }

        let broken = target.exists();

        return Ok( Self { target, path, broken } )

    }
}
