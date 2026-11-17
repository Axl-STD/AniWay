//! Status icon in the panel tray (StatusNotifierItem): shows that overlays run in the background and offers the
//! common actions. Menu actions travel to the event loop as ordinary requests.

use crate::App;
use aniway_proto::ipc::Request;
use aniway_proto::paths;
use calloop::channel::{self, Sender};
use gettextrs::gettext as tr;
use ksni::blocking::TrayMethods;
use ksni::menu::{CheckmarkItem, StandardItem, SubMenu};
use ksni::{MenuItem, ToolTip};

pub type Handle = ksni::blocking::Handle<Tray>;

const ICONS: [(&str, &str); 2] = [
    ("aniway-tray-light", include_str!("../../aniway-editor/resources/icons/aniway-tray-light.svg")),
    ("aniway-tray-dark", include_str!("../../aniway-editor/resources/icons/aniway-tray-dark.svg")),
];

pub struct Tray {
    tx: Sender<Request>,
    hidden: bool,
    paused: bool,
    icons: String,
    icon: &'static str,
}

impl Tray {
    fn send(&self, request: Request) {
        let _ = self.tx.send(request);
    }

    fn open_editor(&mut self) {
        let editor = std::env::current_exe().map(|e| e.with_file_name("aniway-editor"));
        match editor.and_then(|e| std::process::Command::new(e).spawn()) {
            // Reaped on its own thread so that a closed editor never lingers as a zombie.
            Ok(mut child) => drop(std::thread::spawn(move || child.wait())),
            Err(e) => log::warn!("cannot start the editor: {e}"),
        }
    }
}

fn item(label: String, icon: &str, activate: impl Fn(&mut Tray) + Send + 'static) -> MenuItem<Tray> {
    StandardItem { label, icon_name: icon.into(), activate: Box::new(activate), ..Default::default() }.into()
}

fn hide_for(label: String, secs: Option<u64>) -> MenuItem<Tray> {
    item(label, "", move |t| t.send(Request::Hide { id: None, for_secs: secs }))
}

impl ksni::Tray for Tray {
    fn id(&self) -> String {
        "aniway".into()
    }

    fn title(&self) -> String {
        "AniWay".into()
    }

    fn icon_name(&self) -> String {
        self.icon.into()
    }

    fn icon_theme_path(&self) -> String {
        self.icons.clone()
    }

    fn tool_tip(&self) -> ToolTip {
        let state = match (self.hidden, self.paused) {
            (true, _) => tr("Overlays hidden"),
            (false, true) => tr("Overlays paused"),
            _ => tr("Overlays running"),
        };
        ToolTip { title: "AniWay".into(), description: state, ..Default::default() }
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        self.open_editor();
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        let visibility = if self.hidden {
            item(tr("Show overlays"), "view-reveal-symbolic", |t| t.send(Request::Show { id: None }))
        } else {
            SubMenu {
                label: tr("Hide overlays"),
                icon_name: "view-conceal-symbolic".into(),
                submenu: vec![
                    hide_for(tr("Until I show them"), None),
                    hide_for(tr("For 15 minutes"), Some(15 * 60)),
                    hide_for(tr("For 1 hour"), Some(60 * 60)),
                ],
                ..Default::default()
            }
            .into()
        };
        let pause = CheckmarkItem {
            label: tr("Pause all"),
            checked: self.paused,
            activate: Box::new(|t: &mut Self| {
                t.send(if t.paused { Request::Resume { id: None } } else { Request::Pause { id: None } })
            }),
            ..Default::default()
        };
        vec![
            item(tr("Open editor"), "document-edit-symbolic", Self::open_editor),
            MenuItem::Separator,
            pause.into(),
            visibility,
            MenuItem::Separator,
            item(tr("Quit AniWay"), "application-exit-symbolic", |t| t.send(Request::Quit)),
        ]
    }
}

/// Writes the icons where the panel can find them by name; returns the theme directory.
fn install_icons() -> std::io::Result<String> {
    let root = paths::cache_dir().join("icons");
    let dir = root.join("hicolor/scalable/apps");
    std::fs::create_dir_all(&dir)?;
    for (name, svg) in ICONS {
        let file = dir.join(format!("{name}.svg"));
        if std::fs::read_to_string(&file).ok().as_deref() != Some(svg) {
            std::fs::write(file, svg)?;
        }
    }
    Ok(root.to_string_lossy().into_owned())
}

/// Tray icon for the desktop's colour preference. Panels do not tell their colour, so this follows the
/// `color-scheme` setting that themes and shells set; light strokes when it says nothing, as most panels are dark.
fn tray_icon() -> &'static str {
    let scheme = std::process::Command::new("gsettings")
        .args(["get", "org.gnome.desktop.interface", "color-scheme"])
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned());
    if scheme.is_some_and(|s| s.contains("prefer-light")) {
        "aniway-tray-dark"
    } else {
        "aniway-tray-light"
    }
}

impl App {
    /// Starts or stops the tray icon to match `config.tray`.
    pub(crate) fn sync_tray(&mut self) {
        if !self.config.tray {
            if let Some((handle, _)) = self.tray.take() {
                handle.shutdown();
            }
            return;
        }
        if self.tray.is_some() {
            return;
        }
        let (tx, rx) = channel::channel();
        let inserted = self.handle.insert_source(rx, |ev, _, app| {
            if let channel::Event::Msg(request) = ev {
                if let Err(e) = app.exec(request) {
                    log::warn!("{e:#}");
                }
            }
        });
        if inserted.is_err() {
            return;
        }
        let (hidden, paused) = self.tray_state();
        let tray = Tray { tx, hidden, paused, icons: install_icons().unwrap_or_default(), icon: tray_icon() };
        // Panels that start after us still pick the icon up; without any SNI host it simply stays invisible.
        match tray.assume_sni_available(true).spawn() {
            Ok(handle) => self.tray = Some((handle, (hidden, paused))),
            Err(e) => log::warn!("no tray icon: {e}"),
        }
    }

    fn tray_state(&self) -> (bool, bool) {
        (self.hidden || self.snooze.is_some(), self.paused_all)
    }

    /// Mirrors the hidden and paused state in the tray menu (a round trip to its thread, so only on change).
    pub(crate) fn update_tray(&mut self) {
        let now = self.tray_state();
        let Some((handle, shown)) = self.tray.as_mut().filter(|(_, shown)| *shown != now) else { return };
        *shown = now;
        handle.update(|t| (t.hidden, t.paused) = now);
    }
}
