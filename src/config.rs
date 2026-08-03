use std::fs::File;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum AlertScope {
    #[default]
    All,
    Local,
    Remote,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct IdleAlerts {
    pub notification: bool,
    pub sound: bool,
    pub scope: AlertScope,
    pub include_subagents: bool,
    pub sound_file: Option<PathBuf>,
    pub respect_dnd: bool,
    pub dnd_command: Vec<String>,
}

impl Default for IdleAlerts {
    fn default() -> Self {
        Self {
            notification: false,
            sound: false,
            scope: AlertScope::All,
            include_subagents: false,
            sound_file: None,
            respect_dnd: true,
            dnd_command: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub idle_alerts: IdleAlerts,
}

#[derive(Debug)]
pub struct LoadedConfig {
    pub config: Config,
    pub directory: PathBuf,
}

impl LoadedConfig {
    pub fn load() -> Result<Self> {
        Self::load_from(config_path())
    }

    fn load_from(path: PathBuf) -> Result<Self> {
        let directory = path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf();
        if !path.exists() {
            return Ok(Self {
                config: Config::default(),
                directory,
            });
        }
        let file = File::open(&path)
            .with_context(|| format!("failed to open alert config {}", path.display()))?;
        let config = serde_json::from_reader(file)
            .with_context(|| format!("failed to parse alert config {}", path.display()))?;
        Ok(Self { config, directory })
    }
}

fn config_path() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home_dir().join(".config"))
        .join("agent-session-status/config.json")
}

pub fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_disable_all_delivery() {
        let config = Config::default();
        assert!(!config.idle_alerts.notification);
        assert!(!config.idle_alerts.sound);
        assert_eq!(config.idle_alerts.scope, AlertScope::All);
        assert!(!config.idle_alerts.include_subagents);
        assert_eq!(config.idle_alerts.sound_file, None);
        assert!(config.idle_alerts.respect_dnd);
        assert!(config.idle_alerts.dnd_command.is_empty());
    }

    #[test]
    fn parses_schema_and_rejects_bad_values_with_path_context() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        std::fs::write(
            &path,
            r#"{"idle_alerts":{"notification":true,"sound":true,"scope":"remote","include_subagents":true,"sound_file":"tone.wav","respect_dnd":false,"dnd_command":["dnd"]}}"#,
        )
        .unwrap();
        let loaded = LoadedConfig::load_from(path.clone()).unwrap();
        assert_eq!(loaded.config.idle_alerts.scope, AlertScope::Remote);
        assert_eq!(
            loaded.config.idle_alerts.sound_file,
            Some("tone.wav".into())
        );

        std::fs::write(&path, r#"{"idle_alerts":{"scope":"nearby"}}"#).unwrap();
        let error = LoadedConfig::load_from(path.clone()).unwrap_err();
        assert!(error.to_string().contains(path.to_str().unwrap()));
    }

    #[test]
    fn missing_file_uses_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let loaded = LoadedConfig::load_from(dir.path().join("missing.json")).unwrap();
        assert_eq!(loaded.config, Config::default());
    }
}
