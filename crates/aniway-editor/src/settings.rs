//! Settings of the editor alone (theme, language), kept apart from the daemon's config.toml.

use aniway_proto::paths::config_dir;
use serde::{Deserialize, Serialize};

#[derive(Default, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Theme id, or empty to follow the system light or dark style.
    pub theme: String,
    /// Language code (`en`, `es`), or empty for the system language.
    pub language: String,
}

fn file() -> std::path::PathBuf {
    config_dir().join("editor.toml")
}

pub fn load() -> Settings {
    std::fs::read_to_string(file()).ok().and_then(|t| toml::from_str(&t).ok()).unwrap_or_default()
}

pub fn save(settings: &Settings) -> std::io::Result<()> {
    std::fs::create_dir_all(config_dir())?;
    std::fs::write(file(), toml::to_string(settings).unwrap_or_default())
}
