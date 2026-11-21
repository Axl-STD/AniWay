//! Settings of the editor alone (theme, language), kept apart from the daemon's config.toml.

use aniway_proto::paths::{config_dir, expand_tilde, home};
use serde::{Deserialize, Serialize};
use std::io;
use std::path::{Path, PathBuf};

#[derive(Default, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Theme id, or empty to follow the system light or dark style.
    pub theme: String,
    /// Language code (`en`, `es`), or empty for the system language.
    pub language: String,
    /// Folders scanned by the media library.
    pub library: Vec<String>,
}

fn effective_library_folders(settings: &Settings) -> Vec<PathBuf> {
    if !settings.library.is_empty() {
        return settings.library.iter().map(|p| expand_tilde(p)).collect();
    }
    let candidates = [
        home().join(".local/share/Steam/steamapps/workshop/content/3474900"),
        home().join(".var/app/com.valvesoftware.Steam/.local/share/Steam/steamapps/workshop/content/3474900"),
    ];
    candidates.into_iter().filter(|p| p.exists()).collect()
}

/// Returns saved library folders, or the existing Steam Workshop folder by default.
pub fn library_folders() -> Vec<PathBuf> {
    effective_library_folders(&load())
}

/// Adds a folder to the effective library list and saves it.
pub fn add_library_folder(path: &Path) -> io::Result<()> {
    let mut folders = effective_library_folders(&load());
    if !folders.iter().any(|folder| folder == path) {
        folders.push(path.to_path_buf());
    }
    save(&Settings { library: folders.iter().map(|folder| folder.to_string_lossy().into_owned()).collect(), ..load() })
}

/// Removes a folder from the effective library list and saves it.
pub fn remove_library_folder(path: &Path) -> io::Result<()> {
    let current = load();
    let folders: Vec<_> = effective_library_folders(&current).into_iter().filter(|folder| folder != path).collect();
    save(&Settings { library: folders.iter().map(|folder| folder.to_string_lossy().into_owned()).collect(), ..current })
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
