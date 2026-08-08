use directories::ProjectDirs;
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use serde::Deserialize;
use shellexpand;
use std::path::Path;
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
    #[error("Skip pattern error: {0}")]
    SkipPattern(#[from] ignore::Error),
}

pub struct Config {
    pub paths: Paths,
    pub skip: Skip,
}

#[derive(Deserialize)]
struct TOMLConfig {
    paths: TOMLPaths,
    #[serde(default)]
    skip: TOMLSkip,
}

#[derive(Deserialize)]
pub struct TOMLPaths {
    root: String,
    database: String,
}

#[derive(Deserialize, Default)]
struct TOMLSkip {
    #[serde(default)]
    patterns: Vec<String>,
}

pub struct Paths {
    pub root: PathBuf,
    pub database: PathBuf,
}

pub struct Skip {
    matcher: Option<Gitignore>,
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
        let skip = Skip::from_config(tc.skip, &paths.root)?;

        Ok(Self { paths, skip })
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

impl Skip {
    fn from_config(tp: TOMLSkip, root: &Path) -> Result<Self, ConfigError> {
        if tp.patterns.is_empty() {
            return Ok(Self { matcher: None });
        }

        let mut builder = GitignoreBuilder::new(root);
        for pattern in &tp.patterns {
            builder.add_line(None, pattern)?;
        }
        let matcher = builder.build()?;

        Ok(Self {
            matcher: Some(matcher),
        })
    }

    pub fn is_empty(&self) -> bool {
        self.matcher.is_none()
    }

    pub fn matches(&self, rel_path: &Path, is_dir: bool) -> bool {
        match &self.matcher {
            Some(matcher) => matches!(matcher.matched(rel_path, is_dir), ignore::Match::Ignore(_)),
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn empty_patterns_match_nothing() {
        let skip = Skip::from_config(TOMLSkip::default(), Path::new("/tmp")).unwrap();
        assert!(skip.is_empty());
        assert!(!skip.matches(Path::new(".venv"), true));
    }

    #[test]
    fn basename_patterns_match_at_any_depth() {
        let skip = Skip::from_config(
            TOMLSkip {
                patterns: vec![".venv".into(), "target".into(), "node_modules".into()],
            },
            Path::new("/tmp"),
        )
        .unwrap();

        for rel in [".venv", "a/.venv", "a/b/target", "node_modules"] {
            assert!(skip.matches(Path::new(rel), true), "{rel} should match");
        }
        for rel in ["src/main.rs", "a/venv", "Target", "node_modules/x/y"] {
            assert!(
                !skip.matches(Path::new(rel), true),
                "{rel} should not match"
            );
        }
    }

    #[test]
    fn leading_slash_anchors_to_root() {
        let skip = Skip::from_config(
            TOMLSkip {
                patterns: vec!["/target".into()],
            },
            Path::new("/tmp"),
        )
        .unwrap();

        assert!(skip.matches(Path::new("target"), true));
        assert!(!skip.matches(Path::new("a/target"), true));
    }
}
