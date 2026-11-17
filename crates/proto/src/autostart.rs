//! Optional systemd user unit that starts the daemon with the graphical session. Plain file operations, followed
//! by a reload of the user manager: it outlives logouts, so without one it keeps the old unit graph and the next
//! login would not start the daemon.

use crate::paths::xdg_config_home;
use std::io;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};

const UNIT: &str = "aniway.service";
/// Unit of the former name (OverWayland), replaced by [`migrate_legacy`].
const LEGACY_UNIT: &str = "overwayland.service";
const TARGET: &str = "graphical-session.target";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    pub enabled: bool,
    pub exec: Option<PathBuf>,
    pub exec_exists: bool,
}

fn unit_dir(base: &Path) -> PathBuf {
    base.join("systemd/user")
}

fn unit_file(base: &Path) -> PathBuf {
    unit_dir(base).join(UNIT)
}

fn wants_link(base: &Path) -> PathBuf {
    unit_dir(base).join(format!("{TARGET}.wants")).join(UNIT)
}

fn unit_text(exec: &Path) -> String {
    format!(
        "[Unit]\nDescription=AniWay media overlays\nPartOf={TARGET}\nAfter={TARGET}\nRequisite={TARGET}\n\n\
         [Service]\nExecStart=\"{}\"\nRestart=on-failure\nRestartSec=2\nRestartPreventExitStatus=3\n\n\
         [Install]\nWantedBy={TARGET}\n",
        exec.display()
    )
}

/// Writes the unit (atomically) and enables it; safe to repeat, which also refreshes `ExecStart`.
pub fn enable(exec: &Path) -> io::Result<()> {
    enable_in(&xdg_config_home(), exec)?;
    reload();
    Ok(())
}

fn enable_in(base: &Path, exec: &Path) -> io::Result<()> {
    let (unit, link) = (unit_file(base), wants_link(base));
    std::fs::create_dir_all(link.parent().unwrap_or(&unit_dir(base)))?;
    let tmp = unit.with_extension("service.tmp");
    std::fs::write(&tmp, unit_text(exec))?;
    std::fs::rename(&tmp, &unit)?;
    remove_if_exists(&link)?;
    symlink(format!("../{UNIT}"), &link)
}

pub fn disable() -> io::Result<()> {
    disable_in(&xdg_config_home())?;
    reload();
    Ok(())
}

/// Asks the user manager to reread its units, in the background; without systemd there is nothing to tell.
fn reload() {
    let child = std::process::Command::new("systemctl")
        .args(["--user", "daemon-reload"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
    if let Ok(mut child) = child {
        drop(std::thread::spawn(move || child.wait()));
    }
}

fn disable_in(base: &Path) -> io::Result<()> {
    remove_if_exists(&wants_link(base))?;
    remove_if_exists(&unit_file(base))
}

fn remove_if_exists(path: &Path) -> io::Result<()> {
    match std::fs::remove_file(path) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
        _ => Ok(()),
    }
}

/// Replaces a unit of the former name with ours, enabled the same way, running `exec`. Returns whether one existed.
pub fn migrate_legacy(exec: &Path) -> io::Result<bool> {
    let migrated = migrate_legacy_in(&xdg_config_home(), exec)?;
    if migrated {
        reload();
    }
    Ok(migrated)
}

fn migrate_legacy_in(base: &Path, exec: &Path) -> io::Result<bool> {
    let (unit, link) =
        (unit_dir(base).join(LEGACY_UNIT), unit_dir(base).join(format!("{TARGET}.wants")).join(LEGACY_UNIT));
    if !unit.exists() && !link.is_symlink() {
        return Ok(false);
    }
    if link.is_symlink() {
        enable_in(base, exec)?;
    }
    remove_if_exists(&link)?;
    remove_if_exists(&unit)?;
    Ok(true)
}

pub fn status() -> Status {
    status_in(&xdg_config_home())
}

fn status_in(base: &Path) -> Status {
    let text = std::fs::read_to_string(unit_file(base)).ok();
    let exec = text.as_deref().and_then(|t| {
        let value = t.lines().find_map(|l| l.strip_prefix("ExecStart="))?;
        Some(PathBuf::from(value.trim_matches('"')))
    });
    let linked = wants_link(base).is_symlink();
    Status { enabled: text.is_some() && linked, exec_exists: exec.as_ref().is_some_and(|p| p.exists()), exec }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enable_disable_status() {
        let dir = std::env::temp_dir().join(format!("ow-autostart-{}", std::process::id()));
        let exec = dir.join("aniwayd");
        assert_eq!(status_in(&dir), Status { enabled: false, exec: None, exec_exists: false });

        enable_in(&dir, &exec).unwrap();
        enable_in(&dir, &exec).unwrap();
        let text = std::fs::read_to_string(unit_file(&dir)).unwrap();
        assert!(text.contains(&format!("ExecStart=\"{}\"", exec.display())));
        assert!(text.contains("RestartPreventExitStatus=3") && text.contains("WantedBy=graphical-session.target"));
        assert_eq!(std::fs::read_link(wants_link(&dir)).unwrap(), PathBuf::from("../aniway.service"));
        assert_eq!(status_in(&dir), Status { enabled: true, exec: Some(exec.clone()), exec_exists: false });

        std::fs::write(&exec, "").unwrap();
        assert!(status_in(&dir).exec_exists);

        disable_in(&dir).unwrap();
        disable_in(&dir).unwrap();
        assert!(!unit_file(&dir).exists() && !wants_link(&dir).is_symlink());
        assert!(!status_in(&dir).enabled);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn legacy_unit_is_replaced() {
        let dir = std::env::temp_dir().join(format!("aw-legacy-{}", std::process::id()));
        let exec = dir.join("aniwayd");
        assert!(!migrate_legacy_in(&dir, &exec).unwrap());
        let wants = unit_dir(&dir).join(format!("{TARGET}.wants"));
        std::fs::create_dir_all(&wants).unwrap();
        std::fs::write(unit_dir(&dir).join(LEGACY_UNIT), "[Service]\nExecStart=/old/overwaylandd\n").unwrap();
        symlink(format!("../{LEGACY_UNIT}"), wants.join(LEGACY_UNIT)).unwrap();
        assert!(migrate_legacy_in(&dir, &exec).unwrap());
        assert!(!unit_dir(&dir).join(LEGACY_UNIT).exists() && !wants.join(LEGACY_UNIT).is_symlink());
        assert_eq!(status_in(&dir), Status { enabled: true, exec: Some(exec), exec_exists: false });
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
