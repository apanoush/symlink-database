use directories::ProjectDirs;
use serde::Deserialize;
use shellexpand;
use std::{env::VarError, fs, io, path::PathBuf};
use thiserror::Error;
use toml;

#[derive(Error, Debug)]
pub enum ConfigError {
    #[error("Directory not found: {0}")]
    DirectoryNotFound(String),
    #[error("File not found: {0}")]
    FileNotFound(String),
    #[error("ProjectDirs error")]
    ProjectDirsError,
    #[error("IOError: {0}")]
    IOError(#[from] io::Error),
    #[error("TOMLError: {0}")]
    TOMLError(#[from] toml::de::Error),
    #[error("EnvVariableError: {0}")]
    EnvVariableError(#[from] shellexpand::LookupError<VarError>),
    #[error("Config file not found: {0}")]
    ConfigFileNotFound(String),
}

pub struct Config {
    pub paths: Paths,
}

#[derive(Deserialize)]
struct TOMLConfig {
    paths: TOMLPaths,
}

#[derive(Deserialize)]
pub struct TOMLPaths {
    root: String,
    database: String,
}

pub struct Paths {
    pub root: PathBuf,
    pub database: PathBuf,
}

impl Config {
    pub fn from_config_file() -> Result<Self, ConfigError> {
        let proj_dirs = ProjectDirs::from("net", "apanoush", "lndb")
            .ok_or(ConfigError::ProjectDirsError)?
            .config_dir()
            .join("config.toml");

        if !proj_dirs.is_file() {
            return Err(ConfigError::ConfigFileNotFound(
                proj_dirs.to_string_lossy().to_string(),
            ));
        }

        let tc: TOMLConfig = {
            let conf_str = fs::read_to_string(proj_dirs)?;
            toml::from_str::<TOMLConfig>(&conf_str)?
        };

        let paths = Paths::default(tc.paths)?;

        Ok(Self { paths })
    }
}

impl Paths {
    pub fn default(tp: TOMLPaths) -> Result<Self, ConfigError> {
        let root_path = PathBuf::from(shellexpand::env(&tp.root)?.as_ref());
        let database_path = PathBuf::from(shellexpand::env(&tp.database)?.as_ref());

        Ok(Self {
            root: root_path,
            database: database_path,
        })
    }
}
