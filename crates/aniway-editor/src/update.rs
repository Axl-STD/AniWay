//! Updates from the git checkout AniWay was built from: a fetch tells how far behind it is, then `git pull` and
//! install.sh rebuild it; the installer replaces the running daemon (this editor closes with it) and reopens both.

use crate::client::Ctl;
use crate::tr;
use adw::prelude::*;
use aniway_proto::paths::{cache_dir, home};
use gtk::{gio, glib};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::rc::Rc;
use std::time::Duration;

/// The checkout this binary was built from, while it still holds the installer.
fn source() -> Option<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().ok()?;
    (root.join(".git").exists() && root.join("install.sh").is_file()).then_some(root)
}

fn git(root: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Commits upstream has and the checkout lacks; `None` when that cannot be told (offline, no upstream).
async fn behind(root: PathBuf) -> Option<u32> {
    gio::spawn_blocking(move || {
        git(&root, &["fetch", "--quiet"])?;
        git(&root, &["rev-list", "--count", "HEAD..@{u}"])?.parse().ok()
    })
    .await
    .ok()
    .flatten()
}

/// Pulls and reinstalls in the background, logging to the cache; a failure leaves this version running.
fn install(ctl: &Rc<Ctl>, root: &Path) {
    let log = cache_dir().join("update.log");
    let file = std::fs::create_dir_all(cache_dir()).and_then(|()| std::fs::File::create(&log));
    let Ok((out, err)) = file.and_then(|f| Ok((f.try_clone()?, f))) else {
        return ctl.toast(&tr("Cannot write {}").replace("{}", &log.display().to_string()));
    };
    // Launchers start the editor without the shell's PATH, where rustup puts cargo.
    let path = format!("{}:{}", home().join(".cargo/bin").display(), std::env::var("PATH").unwrap_or_default());
    let child = Command::new("sh")
        .args(["-c", "git pull --ff-only && ./install.sh"])
        .current_dir(root)
        .env("PATH", path)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .stdout(out)
        .stderr(err)
        .process_group(0)
        .spawn();
    let mut child = match child {
        Ok(child) => child,
        Err(e) => return ctl.toast(&e.to_string()),
    };
    let progress =
        adw::Toast::builder().title(tr("Updating… AniWay restarts by itself when it is ready")).timeout(0).build();
    ctl.toasts.add_toast(progress.clone());
    let ctl = ctl.clone();
    glib::timeout_add_local(Duration::from_secs(1), move || match child.try_wait() {
        Ok(None) => glib::ControlFlow::Continue,
        Ok(Some(status)) if status.success() => glib::ControlFlow::Break,
        _ => {
            progress.dismiss();
            ctl.toast(&tr("The update failed; details in {}").replace("{}", &log.display().to_string()));
            glib::ControlFlow::Break
        }
    });
}

/// Main menu entry: says whether there is an update and offers to install it.
pub fn check(ctl: &Rc<Ctl>, window: &adw::ApplicationWindow) {
    let Some(root) = source() else {
        let dialog = adw::AlertDialog::new(
            Some(&tr("Updates unavailable")),
            Some(&tr("AniWay updates from the folder it was installed from, which is gone. Install it again with install.sh.")),
        );
        dialog.add_response("close", &tr("Close"));
        return dialog.present(Some(window));
    };
    let checking = adw::Toast::builder().title(tr("Checking for updates…")).timeout(0).build();
    ctl.toasts.add_toast(checking.clone());
    let (ctl, window) = (ctl.clone(), window.clone());
    glib::spawn_future_local(async move {
        let (title, body, update) = match behind(root.clone()).await {
            None => (tr("Cannot check for updates"), tr("Check your connection and try again."), false),
            Some(0) => (tr("AniWay is up to date"), String::new(), false),
            Some(n) => (
                tr("An update is available"),
                tr("{} new changes. Updating takes a few minutes.").replace("{}", &n.to_string()),
                true,
            ),
        };
        checking.dismiss();
        let dialog = adw::AlertDialog::new(Some(&title), Some(&body));
        dialog.add_response("close", &tr("Close"));
        if update {
            dialog.add_response("update", &tr("Update"));
            dialog.set_response_appearance("update", adw::ResponseAppearance::Suggested);
            dialog.set_default_response(Some("update"));
        }
        dialog.choose(Some(&window), gio::Cancellable::NONE, move |r| {
            if r == "update" {
                install(&ctl, &root);
            }
        });
    });
}

/// At start: a toast with an Update button when the checkout is behind; silent otherwise.
pub fn check_quietly(ctl: &Rc<Ctl>) {
    let Some(root) = source() else { return };
    let ctl = ctl.clone();
    glib::spawn_future_local(async move {
        if behind(root.clone()).await.is_some_and(|n| n > 0) {
            let toast =
                adw::Toast::builder().title(tr("An update is available")).button_label(tr("Update")).timeout(0).build();
            ctl.toasts.add_toast(toast.clone());
            toast.connect_button_clicked(move |_| install(&ctl, &root));
        }
    });
}
