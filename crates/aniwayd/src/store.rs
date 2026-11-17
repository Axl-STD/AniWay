use aniway_proto::model::{parse_toml, to_toml, Config, Scene};
use aniway_proto::paths;
use anyhow::{bail, Context, Result};
use serde::de::DeserializeOwned;
use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::fs::{self, File};
use std::hash::{Hash, Hasher};
use std::io::Write;
use std::path::{Path, PathBuf};

/// Reads and writes settings and scenes. The daemon is the only writer of these files.
#[derive(Default)]
pub struct Store {
    /// Hash of the last content read or written per file, used to ignore our own writes.
    known: HashMap<PathBuf, u64>,
}

fn hash(text: &str) -> u64 {
    let mut h = DefaultHasher::new();
    text.hash(&mut h);
    h.finish()
}

impl Store {
    /// Creates the directories and the default files on first run.
    pub fn init(&mut self) -> Result<()> {
        fs::create_dir_all(paths::scenes_dir()).context("creating config directory")?;
        if !paths::config_file().exists() {
            self.write(&paths::config_file(), &to_toml(&Config::default()))?;
        }
        if !paths::scene_file("default").exists() {
            self.write(&paths::scene_file("default"), &to_toml(&Scene::default()))?;
        }
        Ok(())
    }

    /// Returns the file content when it differs from what we last saw (or when `force`).
    pub fn read_changed(&mut self, path: &Path, force: bool) -> Result<Option<String>> {
        let text = fs::read_to_string(path).with_context(|| path.display().to_string())?;
        let new = hash(&text);
        let seen = self.known.insert(path.to_path_buf(), new);
        Ok((force || seen != Some(new)).then_some(text))
    }

    pub fn load_config(&mut self, force: bool) -> Result<Option<Config>> {
        let path = paths::config_file();
        self.read_changed(&path, force)?.map(|text| parse(&path, &text)).transpose()
    }

    pub fn load_scene(&mut self, name: &str, force: bool) -> Result<Option<Scene>> {
        let path = paths::scene_file(name);
        let mut scene: Option<Scene> = self.read_changed(&path, force)?.map(|text| parse(&path, &text)).transpose()?;
        if let Some(s) = &mut scene {
            s.normalize();
        }
        Ok(scene)
    }

    pub fn save_config(&mut self, config: &Config) -> Result<()> {
        self.write(&paths::config_file(), &to_toml(config))
    }

    pub fn save_scene(&mut self, name: &str, scene: &Scene) -> Result<()> {
        self.write(&paths::scene_file(name), &to_toml(scene))
    }

    pub fn scene_names(&self) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(paths::scenes_dir())
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| {
                e.path()
                    .file_stem()
                    .zip(e.path().extension().filter(|x| *x == "toml"))
                    .map(|(s, _)| s.to_string_lossy().into_owned())
            })
            .filter(|name| paths::valid_scene_name(name))
            .collect();
        names.sort();
        names
    }

    pub fn rename_scene(&mut self, from: &str, to: &str) -> Result<()> {
        let (old, new) = (paths::scene_file(from), paths::scene_file(to));
        if let Some(hash) = self.known.remove(&old) {
            self.known.insert(new.clone(), hash);
        }
        fs::rename(&old, &new).with_context(|| old.display().to_string())
    }

    pub fn delete_scene(&mut self, name: &str) -> Result<()> {
        let path = paths::scene_file(name);
        if !path.exists() {
            bail!("scene '{name}' does not exist");
        }
        self.known.remove(&path);
        fs::remove_file(&path).with_context(|| path.display().to_string())
    }

    /// Writes atomically (temp file, fsync, rename) and records the hash to suppress the watcher echo.
    fn write(&mut self, path: &Path, text: &str) -> Result<()> {
        // Hidden and suffixed, so a half-written file is never listed as a scene; any dots in the name are kept.
        let tmp = path.with_file_name(format!(".{}.tmp", path.file_name().unwrap_or_default().to_string_lossy()));
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).with_context(|| dir.display().to_string())?;
        }
        let mut file = File::create(&tmp).with_context(|| tmp.display().to_string())?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        self.known.insert(path.to_path_buf(), hash(text));
        fs::rename(&tmp, path).with_context(|| path.display().to_string())
    }
}

fn parse<T: DeserializeOwned>(path: &Path, text: &str) -> Result<T> {
    let (value, unknown) = parse_toml(text).with_context(|| format!("{}", path.display()))?;
    for key in unknown {
        log::warn!("{}: unknown key '{key}' ignored", path.display());
    }
    Ok(value)
}
