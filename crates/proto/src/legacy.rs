//! One-time move of the files of the former name, OverWayland, to AniWay's: configuration and scenes, the cache and
//! the optional autostart unit. Every entry point runs it; whichever comes first does the work.

use crate::{autostart, paths};
use std::path::Path;

const LEGACY: &str = "overwayland";

/// Moves what exists under the former name; `daemon` is the `aniwayd` an old autostart unit should run instead.
/// Returns one line per thing moved or failed, for the log.
pub fn migrate(daemon: Option<&Path>) -> Vec<String> {
    let mut notes = Vec::new();
    for (old, new) in [
        (paths::xdg_config_home().join(LEGACY), paths::config_dir()),
        (paths::xdg_cache_home().join(LEGACY), paths::cache_dir()),
    ] {
        if old.is_dir() && !new.exists() {
            notes.push(match std::fs::rename(&old, &new) {
                Ok(()) => format!("moved {} to {}", old.display(), new.display()),
                Err(e) => format!("cannot move {} to {}: {e}", old.display(), new.display()),
            });
        }
    }
    if let Some(daemon) = daemon {
        match autostart::migrate_legacy(daemon) {
            Ok(true) => notes.push("replaced the overwayland.service autostart unit with aniway.service".into()),
            Ok(false) => {}
            Err(e) => notes.push(format!("cannot replace the overwayland.service autostart unit: {e}")),
        }
    }
    notes
}
