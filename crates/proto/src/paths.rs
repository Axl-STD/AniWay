use std::path::PathBuf;

fn env_path(var: &str) -> Option<PathBuf> {
    std::env::var_os(var).filter(|v| !v.is_empty()).map(PathBuf::from)
}

pub fn home() -> PathBuf {
    env_path("HOME").unwrap_or_else(|| PathBuf::from("/"))
}

pub fn xdg_config_home() -> PathBuf {
    env_path("XDG_CONFIG_HOME").unwrap_or_else(|| home().join(".config"))
}

pub fn config_dir() -> PathBuf {
    xdg_config_home().join("aniway")
}

pub fn config_file() -> PathBuf {
    config_dir().join("config.toml")
}

pub fn scenes_dir() -> PathBuf {
    config_dir().join("scenes")
}

pub fn scene_file(name: &str) -> PathBuf {
    scenes_dir().join(format!("{name}.toml"))
}

pub fn xdg_cache_home() -> PathBuf {
    env_path("XDG_CACHE_HOME").unwrap_or_else(|| home().join(".cache"))
}

pub fn cache_dir() -> PathBuf {
    xdg_cache_home().join("aniway")
}

/// Directory of the cached optimized copies of media.
pub fn proxies_dir() -> PathBuf {
    cache_dir().join("proxies")
}

fn runtime_dir() -> PathBuf {
    env_path("XDG_RUNTIME_DIR").unwrap_or_else(std::env::temp_dir)
}

pub fn socket_path() -> PathBuf {
    runtime_dir().join("aniway.sock")
}

pub fn lock_path() -> PathBuf {
    runtime_dir().join("aniway.lock")
}

/// Longest scene name, in characters.
pub const SCENE_NAME_MAX: usize = 64;

/// Scene names become file names: any printable text without '/', not hidden, not padded with spaces.
pub fn valid_scene_name(name: &str) -> bool {
    !name.is_empty()
        && name.trim() == name
        && !name.starts_with('.')
        && name.chars().count() <= SCENE_NAME_MAX
        && !name.chars().any(|c| c == '/' || c.is_control())
}

/// Lower-case file extension of a path or URL, without the dot.
pub fn extension(path: &str) -> String {
    let name = path.rsplit('/').next().unwrap_or(path);
    name.rsplit_once('.').map_or(String::new(), |(_, e)| e.to_ascii_lowercase())
}

/// Expands a leading `~` or `~/` to the home directory.
pub fn expand_tilde(path: &str) -> PathBuf {
    match path.strip_prefix('~') {
        Some("") => home(),
        Some(rest) if rest.starts_with('/') => home().join(&rest[1..]),
        _ => PathBuf::from(path),
    }
}

pub fn is_url(path: &str) -> bool {
    path.split_once("://")
        .is_some_and(|(scheme, _)| !scheme.is_empty() && scheme.chars().all(|c| c.is_ascii_alphanumeric()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scene_names_are_restricted() {
        assert!(valid_scene_name("work-2_a"));
        assert!(valid_scene_name("Escena 1.5 — noche"));
        assert!(!valid_scene_name(""));
        assert!(!valid_scene_name(" padded"));
        assert!(!valid_scene_name(".hidden"));
        assert!(!valid_scene_name("tab\tname"));
        assert!(!valid_scene_name("../etc"));
        assert!(!valid_scene_name("a/b"));
    }

    #[test]
    fn tilde_and_url() {
        assert_eq!(expand_tilde("/a/b"), PathBuf::from("/a/b"));
        assert_eq!(expand_tilde("~/x.gif"), home().join("x.gif"));
        assert_eq!(expand_tilde("~other/x"), PathBuf::from("~other/x"));
        assert_eq!(extension("/a.b/C.GIF"), "gif");
        assert_eq!(extension("/a.b/noext"), "");
        assert!(is_url("https://a/b.gif"));
        assert!(!is_url("/tmp/a://b"));
    }
}
