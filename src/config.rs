use serde::Deserialize;
use thiserror::Error;
use std::{env::VarError, fs, io, path::{PathBuf, StripPrefixError}};
use toml;
use directories::ProjectDirs;
use shellexpand;

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
    ConfigFileNotFound(String)
}

pub struct Config {
	pub paths: Paths,
}

#[derive(Deserialize)]
pub struct TOMLPaths {
    root_path: String,
    database_path: String,
}

pub struct Paths {
	pub root_path: PathBuf,
	pub database_path: PathBuf,
}

impl Config {

    pub fn from_config_file() -> Result<Self, ConfigError> {
        let proj_dirs = ProjectDirs::from("net", "apanoush", "lndb")
            .ok_or(ConfigError::ProjectDirsError)?
            .config_dir().join("config.toml");

        if ! proj_dirs.is_file() {
            return Err(ConfigError::ConfigFileNotFound(proj_dirs.to_string_lossy().to_string()));
        }

        let tp: TOMLPaths = {
            let conf_str = fs::read_to_string(proj_dirs)?;
            toml::from_str::<TOMLPaths>(&conf_str)?
        };

        let paths = Paths::default(tp)?;

        Ok(Self{paths})
    }
}

impl Paths {

	pub fn default(tp: TOMLPaths) -> Result<Self, ConfigError> {

		let root_path = PathBuf::from(shellexpand::env(&tp.root_path)?.as_ref());
		let database_path = PathBuf::from(shellexpand::env(&tp.database_path)?.as_ref());

		Ok(Self{
			root_path,
            database_path,
		})
	}
}

