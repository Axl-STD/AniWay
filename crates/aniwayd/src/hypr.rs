use aniway_proto::model::Config;
use anyhow::{bail, Context, Result};
use serde_json::Value;
use std::collections::HashSet;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_millis(200);
const RELEVANT: [&str; 9] = [
    "workspace",
    "focusedmon",
    "activewindow",
    "openwindow",
    "closewindow",
    "movewindow",
    "monitoradded",
    "monitorremoved",
    "changefloatingmode",
];

pub struct Hypr {
    dir: PathBuf,
}

/// Monitor names whose focused window is in `hide_for_classes`.
#[derive(Default, PartialEq, Eq)]
pub struct Screen {
    pub class_hidden: HashSet<String>,
}

/// Lua table of the binds this daemon made.
const BINDS: &str = "aniway_binds";

/// Modifier mask and lower-case key of "SUPER + SHIFT + F1", as Hyprland reports its binds.
pub fn combo(keys: &str) -> (i64, String) {
    let mut parts: Vec<String> = keys.split('+').map(|p| p.trim().to_ascii_lowercase()).collect();
    let key = parts.pop().unwrap_or_default();
    let mask = parts.iter().map(|m| match m.as_str() {
        "shift" => 1,
        "ctrl" => 4,
        "alt" => 8,
        "super" => 64,
        _ => 0,
    });
    (mask.fold(0, |a, b| a | b), key)
}

/// True for the socket2 events that can change which window has focus on which monitor.
pub fn is_relevant(line: &str) -> bool {
    let name = line.split_once(">>").map_or(line, |(n, _)| n);
    RELEVANT.iter().any(|r| name.starts_with(r))
}

impl Hypr {
    pub fn detect() -> Option<Self> {
        let signature = std::env::var("HYPRLAND_INSTANCE_SIGNATURE").ok().filter(|s| !s.is_empty())?;
        let runtime = std::env::var_os("XDG_RUNTIME_DIR")?;
        Some(Self { dir: PathBuf::from(runtime).join("hypr").join(signature) })
    }

    pub fn events(&self) -> Result<UnixStream> {
        let stream = UnixStream::connect(self.dir.join(".socket2.sock")).context("Hyprland event socket")?;
        stream.set_nonblocking(true)?;
        Ok(stream)
    }

    fn send(&self, command: &str) -> Result<String> {
        let mut stream = UnixStream::connect(self.dir.join(".socket.sock")).context("Hyprland request socket")?;
        stream.set_read_timeout(Some(TIMEOUT))?;
        stream.set_write_timeout(Some(TIMEOUT))?;
        stream.write_all(command.as_bytes())?;
        let mut text = String::new();
        stream.read_to_string(&mut text)?;
        Ok(text)
    }

    fn request(&self, command: &str) -> Result<Value> {
        serde_json::from_str(&self.send(command)?).with_context(|| format!("Hyprland reply to {command}"))
    }

    /// Binds `keys` to run the shell `command`, replacing our earlier bind of them; `None` only removes it. Lua
    /// configs keep our binds in a table, so removing one never touches a user bind of the same keys.
    pub fn bind(&self, keys: &str, command: Option<&str>) -> Result<()> {
        let k = format!("{keys:?}");
        let mut code =
            format!("{BINDS} = {BINDS} or {{}}; if {BINDS}[{k}] then {BINDS}[{k}]:remove() end; {BINDS}[{k}] = nil");
        if let Some(command) = command {
            code += &format!("; {BINDS}[{k}] = hl.bind({k}, hl.dsp.exec_cmd({command:?}))");
        }
        let reply = match self.send(&format!("eval {code}"))? {
            r if r.trim() == "unknown request" => self.bind_legacy(keys, command)?,
            r => r,
        };
        match reply.trim() {
            "ok" => Ok(()),
            error => bail!("{}", error.trim_start_matches("error: ")),
        }
    }

    /// The same through `keyword`, for hyprlang configs (before Hyprland 0.55).
    fn bind_legacy(&self, keys: &str, command: Option<&str>) -> Result<String> {
        let (mods, key) = keys.rsplit_once('+').unwrap_or(("", keys));
        let mods = mods.split('+').map(str::trim).collect::<Vec<_>>().join(" ");
        let reply = self.send(&format!("keyword unbind {mods}, {}", key.trim()))?;
        match command {
            Some(command) => self.send(&format!("keyword bind {mods}, {}, exec, {command}", key.trim())),
            None => Ok(reply),
        }
    }

    /// Key combinations Hyprland binds now, in the form of [`combo`].
    pub fn binds(&self) -> Result<HashSet<(i64, String)>> {
        let binds = self.request("j/binds")?;
        Ok(binds
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|b| Some((b["modmask"].as_i64()?, b["key"].as_str()?.to_ascii_lowercase())))
            .collect())
    }

    /// Name of the monitor that currently has focus.
    pub fn focused_monitor(&self) -> Option<String> {
        let monitors = self.request("j/monitors").ok()?;
        let focused = monitors.as_array()?.iter().find(|m| m["focused"].as_bool() == Some(true))?;
        focused["name"].as_str().map(String::from)
    }

    pub fn screen(&self, config: &Config) -> Result<Screen> {
        let (monitors, active) = (self.request("j/monitors")?, self.request("j/activewindow")?);
        let mut screen = Screen::default();
        for monitor in monitors.as_array().into_iter().flatten() {
            let Some(name) = monitor["name"].as_str() else {
                continue;
            };
            let class = active["class"].as_str().unwrap_or_default();
            if active["monitor"].as_i64().is_some_and(|m| Some(m) == monitor["id"].as_i64())
                && config.hide_for_classes.iter().any(|c| c == class)
            {
                screen.class_hidden.insert(name.to_string());
            }
        }
        Ok(screen)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relevant_events() {
        assert!(is_relevant("workspace>>2"));
        assert!(!is_relevant("fullscreen>>1"));
        assert!(is_relevant("activewindowv2>>abc"));
        assert!(is_relevant("monitoraddedv2>>1,HEADLESS-2,x"));
        assert!(!is_relevant("windowtitle>>abc"));
        assert!(!is_relevant("configreloaded>>"));
    }

    #[test]
    fn combos_match_hyprland_masks() {
        assert_eq!(combo("SUPER + SHIFT + F1"), (65, "f1".into()));
        assert_eq!(combo("ctrl+alt+a"), (12, "a".into()));
        assert_eq!(combo("F13"), (0, "f13".into()));
    }
}
