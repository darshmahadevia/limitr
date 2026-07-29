use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Default, Deserialize, Serialize)]
pub(crate) struct Config {
    #[serde(default)]
    profiles: Vec<AccountProfile>,
}

impl Config {
    pub(crate) fn load(path: &Path) -> Result<Option<Self>, ConfigError> {
        match fs::read_to_string(path) {
            Ok(contents) => {
                let config: Self = toml::from_str(&contents).map_err(ConfigError::Invalid)?;
                config.validate()?;
                Ok(Some(config))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(ConfigError::Read(error)),
        }
    }

    pub(crate) fn add(&mut self, label: String, codex_home: PathBuf) -> Result<(), ConfigError> {
        self.validate_candidate(&label, &codex_home)?;
        self.profiles.push(AccountProfile { label, codex_home });
        Ok(())
    }

    fn validate(&self) -> Result<(), ConfigError> {
        let mut validated = Self::default();
        for profile in &self.profiles {
            validated.validate_candidate(&profile.label, &profile.codex_home)?;
            validated.profiles.push(profile.clone());
        }
        Ok(())
    }

    fn validate_candidate(&self, label: &str, codex_home: &Path) -> Result<(), ConfigError> {
        if !codex_home.is_absolute() {
            return Err(ConfigError::CodexHomeNotAbsolute(codex_home.to_path_buf()));
        }
        if self.profiles.iter().any(|profile| profile.label == label) {
            return Err(ConfigError::DuplicateProfileLabel(label.into()));
        }
        if let Some(profile) = self
            .profiles
            .iter()
            .find(|profile| profile.codex_home == codex_home)
        {
            return Err(ConfigError::DuplicateCodexHome {
                codex_home: codex_home.to_path_buf(),
                label: profile.label.clone(),
            });
        }
        Ok(())
    }

    pub(crate) fn remove(&mut self, label: &str) -> Result<(), ConfigError> {
        let original_len = self.profiles.len();
        self.profiles.retain(|profile| profile.label != label);
        if self.profiles.len() == original_len {
            return Err(ConfigError::ProfileNotFound(label.into()));
        }
        Ok(())
    }

    pub(crate) fn profiles(&self) -> &[AccountProfile] {
        &self.profiles
    }

    pub(crate) fn into_profiles(self) -> Vec<AccountProfile> {
        self.profiles
    }

    pub(crate) fn save(&self, path: &Path) -> Result<(), ConfigError> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(ConfigError::Write)?;
        }
        let contents = toml::to_string_pretty(self).map_err(ConfigError::Encode)?;
        fs::write(path, contents).map_err(ConfigError::Write)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct AccountProfile {
    pub(crate) label: String,
    pub(crate) codex_home: PathBuf,
}

pub(crate) fn config_file() -> Result<PathBuf, ConfigError> {
    let root = config_root().ok_or(ConfigError::DirectoryUnavailable)?;
    Ok(root.join("limitr").join("config.toml"))
}

fn config_root() -> Option<PathBuf> {
    #[cfg(unix)]
    if let Some(root) = env::var_os("XDG_CONFIG_HOME").filter(|value| !value.is_empty()) {
        return Some(PathBuf::from(root));
    }

    platform_config_root()
}

#[cfg(target_os = "windows")]
fn platform_config_root() -> Option<PathBuf> {
    env::var_os("APPDATA")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

#[cfg(target_os = "macos")]
fn platform_config_root() -> Option<PathBuf> {
    home_directory().map(|home| home.join("Library").join("Application Support"))
}

#[cfg(all(unix, not(target_os = "macos")))]
fn platform_config_root() -> Option<PathBuf> {
    home_directory().map(|home| home.join(".config"))
}

#[cfg(unix)]
fn home_directory() -> Option<PathBuf> {
    env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

#[derive(Debug, Error)]
pub(crate) enum ConfigError {
    #[error("could not determine the platform configuration directory")]
    DirectoryUnavailable,
    #[error("Codex Home must be an absolute path: {}", .0.display())]
    CodexHomeNotAbsolute(PathBuf),
    #[error("an Account Profile labelled `{0}` is already configured")]
    DuplicateProfileLabel(String),
    #[error("Codex Home {} is already used by Account Profile `{label}`", codex_home.display())]
    DuplicateCodexHome { codex_home: PathBuf, label: String },
    #[error("no Account Profile labelled `{0}` is configured")]
    ProfileNotFound(String),
    #[error("could not read Limitr configuration: {0}")]
    Read(std::io::Error),
    #[error("Limitr configuration is not valid TOML: {0}")]
    Invalid(toml::de::Error),
    #[error("could not encode Limitr configuration: {0}")]
    Encode(toml::ser::Error),
    #[error("could not write Limitr configuration: {0}")]
    Write(std::io::Error),
}
