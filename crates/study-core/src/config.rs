use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::Result;
use crate::paths;

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub moodle: Moodle,
    pub teams: Teams,
    pub ui: Ui,
    /// `"<source>:<remote_id>"` (a course node id) → short alias.
    pub aliases: BTreeMap<String, String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Moodle {
    pub url: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Teams {
    pub client_id: String,
    pub authority: String,
}

impl Default for Teams {
    fn default() -> Self {
        Self {
            // The Microsoft Teams first-party client; see SPEC §5.2 for why.
            client_id: "1fec8e78-bce4-4aaf-ab1b-5451cc387264".to_string(),
            authority: "https://login.microsoftonline.com/organizations".to_string(),
        }
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Ui {
    pub hidden_courses: Vec<String>,
}

impl Config {
    pub fn alias_for(&self, key: &str) -> Option<&str> {
        self.aliases.get(key).map(String::as_str)
    }

    pub fn set_alias(&mut self, key: &str, alias: &str) {
        self.aliases.insert(key.to_string(), alias.to_string());
    }
}

pub fn default_path() -> PathBuf {
    paths::config_dir().join("config.toml")
}

/// A missing file is not an error: first run simply has no config yet.
pub fn load(path: &Path) -> Result<Config> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Config::default()),
        Err(err) => return Err(err.into()),
    };
    Ok(toml::from_str(&text)?)
}

pub fn save(config: &Config, path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, toml::to_string(config)?)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("config.toml");
        let mut config = Config::default();
        config.moodle.url = "https://moodle.example.edu".to_string();
        config.ui.hidden_courses.push("moodle:1".to_string());
        config.set_alias("moodle:4821", "algo");

        save(&config, &path).unwrap();
        let loaded = load(&path).unwrap();

        assert_eq!(loaded.moodle.url, config.moodle.url);
        assert_eq!(loaded.teams.client_id, config.teams.client_id);
        assert_eq!(loaded.teams.authority, config.teams.authority);
        assert_eq!(loaded.ui.hidden_courses, config.ui.hidden_courses);
        assert_eq!(loaded.aliases, config.aliases);
    }

    #[test]
    fn missing_file_gives_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let config = load(&dir.path().join("config.toml")).unwrap();
        assert_eq!(config.moodle.url, "");
        assert_eq!(
            config.teams.client_id,
            "1fec8e78-bce4-4aaf-ab1b-5451cc387264"
        );
        assert!(config.aliases.is_empty());
    }

    #[test]
    fn partial_toml_fills_teams_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        fs::write(&path, "[moodle]\nurl = \"x\"\n").unwrap();

        let config = load(&path).unwrap();

        assert_eq!(config.moodle.url, "x");
        assert_eq!(
            config.teams.client_id,
            "1fec8e78-bce4-4aaf-ab1b-5451cc387264"
        );
        assert_eq!(
            config.teams.authority,
            "https://login.microsoftonline.com/organizations"
        );
        assert!(config.ui.hidden_courses.is_empty());
    }

    #[test]
    fn set_and_get_alias() {
        let mut config = Config::default();
        assert_eq!(config.alias_for("moodle:4821"), None);
        config.set_alias("moodle:4821", "algo");
        assert_eq!(config.alias_for("moodle:4821"), Some("algo"));
        config.set_alias("moodle:4821", "algo2");
        assert_eq!(config.alias_for("moodle:4821"), Some("algo2"));
    }
}
