//! Preferences dialog: appearance and language of the editor (editor.toml), settings of config.toml through
//! `config_set`, and the "Start at login" unit.

use crate::client::Ctl;
use crate::inspector::{combo, spin, switch};
use crate::settings::{self, Settings};
use crate::theme::{self, Theme, SYSTEM_DARK, SYSTEM_LIGHT, THEMES};
use crate::tr;
use adw::prelude::*;
use aniway_proto::autostart;
use aniway_proto::ipc::Request;
use gtk::{gio, glib};
use serde_json::{json, Value};
use std::cell::{Cell, RefCell};
use std::process::Command;
use std::rc::Rc;

const HWDEC: [&str; 4] = ["auto-safe", "no", "nvdec", "vaapi"];
const FORMATS: [&str; 2] = ["auto", "compressed"];
const LANGUAGES: [&str; 3] = ["", "en", "es"];

/// The `aniwayd` installed next to this editor.
pub fn sibling_daemon() -> Option<std::path::PathBuf> {
    std::env::current_exe().ok()?.with_file_name("aniwayd").canonicalize().ok()
}

pub struct Prefs {
    dialog: adw::PreferencesDialog,
    ctl: Rc<Ctl>,
    updating: Cell<bool>,
    settings: RefCell<Settings>,
    themes: Vec<gtk::ToggleButton>,
    language: adw::ComboRow,
    tray: adw::SwitchRow,
    login: adw::SwitchRow,
    hwdec: adw::ComboRow,
    custom: adw::EntryRow,
    min_height: adw::SpinRow,
    classes: adw::PreferencesGroup,
    class_rows: RefCell<Vec<adw::ActionRow>>,
    optimize: adw::SwitchRow,
    format: adw::ComboRow,
    raw_mb: adw::SpinRow,
    cache_mb: adw::SpinRow,
    library: adw::PreferencesGroup,
    library_rows: RefCell<Vec<adw::ActionRow>>,
}

fn entry(title: &str) -> adw::EntryRow {
    adw::EntryRow::builder().title(title).show_apply_button(true).build()
}

impl Prefs {
    pub fn new(ctl: Rc<Ctl>) -> Rc<Self> {
        let this = Rc::new(Self {
            dialog: adw::PreferencesDialog::builder().title(tr("Preferences")).search_enabled(false).build(),
            ctl,
            updating: Cell::default(),
            settings: RefCell::new(settings::load()),
            themes: (0..=THEMES.len())
                .map(|_| gtk::ToggleButton::builder().css_classes(["flat", "theme-card"]).build())
                .collect(),
            language: combo(&tr("Language"), &[&tr("System default"), "English", "Español"]),
            tray: switch(&tr("Show in the panel tray")),
            login: switch(&tr("Start at login")),
            hwdec: combo(
                &tr("Hardware decoding"),
                &[&tr("Automatic (safe)"), &tr("Off"), "NVDEC", "VA-API", &tr("Other…")],
            ),
            custom: entry(&tr("Decoder name (mpv hwdec value)")),
            min_height: spin(&tr("Minimum video height for hardware decoding"), 1.0, 8640.0, 10.0, 0),
            classes: adw::PreferencesGroup::builder()
                .title(tr("Hide for apps"))
                .description(tr("Overlays on a monitor hide while a window of one of these classes is focused there."))
                .build(),
            class_rows: RefCell::default(),
            optimize: switch(&tr("Allow optimized copies")),
            format: combo(&tr("Copy format"), &[&tr("Uncompressed if it fits"), &tr("Always compressed")]),
            raw_mb: spin(&tr("Largest uncompressed copy per file (MB)"), 1.0, 65536.0, 64.0, 0),
            cache_mb: spin(&tr("Cache size (MB)"), 1.0, 1048576.0, 256.0, 0),
            library: adw::PreferencesGroup::builder()
                .title(tr("Library folders"))
                .description(tr("Folders scanned for media in the Library view."))
                .build(),
            library_rows: RefCell::default(),
        });
        this.build();
        this.wire();
        let weak = Rc::downgrade(&this);
        this.ctl.on_change(move || weak.upgrade().map_or((), |p| p.update()));
        this
    }

    /// Theme cards: a small picture of the window in each theme; the first follows the system.
    fn build_themes(&self) -> gtk::FlowBox {
        let names = [
            tr("System"),
            tr("Graphite coral"),
            tr("Graphite amber"),
            tr("Midnight violet"),
            tr("Midnight mint"),
            tr("Midnight white"),
            tr("Paper coral"),
            tr("Paper blue"),
        ];
        let cards = gtk::FlowBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .min_children_per_line(2)
            .max_children_per_line(4)
            .column_spacing(6)
            .row_spacing(6)
            .homogeneous(true)
            .build();
        for (i, (button, name)) in self.themes.iter().zip(names).enumerate() {
            let art =
                gtk::DrawingArea::builder().content_width(96).content_height(60).halign(gtk::Align::Start).build();
            art.set_draw_func(move |_, cr, w, h| match i {
                0 => {
                    swatch(cr, &THEMES[SYSTEM_DARK], 0.0, f64::from(w) / 2.0, f64::from(h));
                    swatch(cr, &THEMES[SYSTEM_LIGHT], f64::from(w) / 2.0, f64::from(w) / 2.0, f64::from(h));
                }
                _ => swatch(cr, &THEMES[i - 1], 0.0, f64::from(w), f64::from(h)),
            });
            let content = gtk::Box::new(gtk::Orientation::Vertical, 6);
            content.append(&art);
            let label =
                gtk::Label::builder().label(&name).xalign(0.0).ellipsize(gtk::pango::EllipsizeMode::End).build();
            content.append(&label);
            button.set_child(Some(&content));
            button.set_group(self.themes.first().filter(|_| i > 0));
            button.update_property(&[gtk::accessible::Property::Label(&name)]);
            cards.append(button);
        }
        cards
    }

    fn build(self: &Rc<Self>) {
        let page = adw::PreferencesPage::new();
        let appearance = adw::PreferencesGroup::builder().title(tr("Appearance")).build();
        appearance.add(&self.build_themes());
        let languages = adw::PreferencesGroup::builder().margin_top(12).build();
        self.language.set_subtitle(&tr("Takes effect when the editor restarts."));
        languages.add(&self.language);
        page.add(&appearance);
        page.add(&languages);
        let group = |title: &str, description: &str, rows: &[&gtk::Widget]| {
            let g = adw::PreferencesGroup::builder().title(title).description(description).build();
            rows.iter().for_each(|r| g.add(*r));
            page.add(&g);
        };
        self.tray.set_subtitle(&tr("An icon that shows AniWay runs in the background, with pause, hide and quit."));
        group(&tr("General"), "", &[self.login.upcast_ref(), self.tray.upcast_ref()]);
        group(
            &tr("Video decoding"),
            "",
            &[self.hwdec.upcast_ref(), self.custom.upcast_ref(), self.min_height.upcast_ref()],
        );
        let add = entry(&tr("Add a window class"));
        let ctl = self.ctl.clone();
        add.connect_apply(move |row| {
            let class = row.text().trim().to_string();
            if !class.is_empty() {
                let all = [classes(&ctl), vec![class]].concat();
                ctl.send(Request::ConfigSet { patch: json!({ "hide_for_classes": all }) });
                row.set_text("");
            }
        });
        self.classes.add(&add);
        page.add(&self.classes);
        self.optimize.set_subtitle(&tr(
            "Optimized copies of media drawn much smaller. Uncompressed copies cost disk space instead of CPU.",
        ));
        group(
            &tr("Optimized copies"),
            "",
            &[
                self.optimize.upcast_ref(),
                self.format.upcast_ref(),
                self.raw_mb.upcast_ref(),
                self.cache_mb.upcast_ref(),
            ],
        );
        self.build_library();
        page.add(&self.library);
        self.dialog.add(&page);
    }

    fn build_library(self: &Rc<Self>) {
        let add = adw::ActionRow::builder().title(tr("Add folder")).activatable(true).build();
        add.add_suffix(&gtk::Image::from_icon_name("list-add-symbolic"));
        let this = Rc::downgrade(self);
        add.connect_activated(move |_| {
            let Some(this) = this.upgrade() else { return };
            let dialog = gtk::FileDialog::new();
            dialog.select_folder(None::<&gtk::Window>, gio::Cancellable::NONE, move |r| {
                if let Ok(folder) = r {
                    if let Some(path) = folder.path() {
                        if let Err(error) = settings::add_library_folder(&path) {
                            this.ctl.toast(&error.to_string());
                        } else {
                            this.settings.replace(settings::load());
                            this.rebuild_library();
                        }
                    }
                }
            });
        });
        self.library.add(&add);
        self.rebuild_library();
    }

    fn rebuild_library(self: &Rc<Self>) {
        self.library_rows.borrow_mut().drain(..).for_each(|row| self.library.remove(&row));
        let folders = settings::library_folders();
        for path in folders {
            let text = path.to_string_lossy().into_owned();
            let row = adw::ActionRow::builder().title(&text).build();
            let remove = gtk::Button::builder()
                .icon_name("edit-delete-symbolic")
                .css_classes(["flat"])
                .tooltip_text(tr("Remove"))
                .build();
            let this = Rc::downgrade(self);
            let path = path.clone();
            remove.connect_clicked(move |_| {
                if let Some(this) = this.upgrade() {
                    if let Err(error) = settings::remove_library_folder(&path) {
                        this.ctl.toast(&error.to_string());
                    } else {
                        this.settings.replace(settings::load());
                        this.rebuild_library();
                    }
                }
            });
            row.add_suffix(&remove);
            self.library.add(&row);
            self.library_rows.borrow_mut().push(row);
        }
    }

    fn save_settings(&self) {
        if let Err(e) = settings::save(&self.settings.borrow()) {
            self.ctl.toast(&e.to_string());
        }
    }

    fn set(&self, key: &str, value: Value) {
        if !self.updating.get() {
            self.ctl.send(Request::ConfigSet { patch: json!({ key: value }) });
        }
    }

    fn wire(self: &Rc<Self>) {
        macro_rules! on {
            ($w:ident, $signal:ident, $key:literal, |$r:ident| $value:expr) => {
                self.$w.$signal(glib::clone!(
                    #[weak(rename_to = this)]
                    self,
                    move |$r| this.set($key, json!($value))
                ))
            };
        }
        on!(tray, connect_active_notify, "tray", |r| r.is_active());
        for (i, button) in self.themes.iter().enumerate() {
            let this = Rc::downgrade(self);
            button.connect_toggled(move |b| {
                let Some(this) = this.upgrade().filter(|t| b.is_active() && !t.updating.get()) else { return };
                let id = if i == 0 { "" } else { THEMES[i - 1].id };
                this.settings.borrow_mut().theme = id.to_string();
                this.save_settings();
                theme::apply(id);
            });
        }
        // xgettext skips strings inside macro calls, so these are built outside `clone!`.
        let (restart_note, restart_label) = (tr("The new language shows after a restart"), tr("Restart"));
        self.language.connect_selected_notify(glib::clone!(
            #[weak(rename_to = this)]
            self,
            move |r| {
                if this.updating.get() {
                    return;
                }
                this.settings.borrow_mut().language = LANGUAGES.get(r.selected() as usize).unwrap_or(&"").to_string();
                this.save_settings();
                let toast =
                    adw::Toast::builder().title(restart_note.as_str()).button_label(restart_label.as_str()).build();
                toast.connect_button_clicked(|_| restart());
                this.dialog.add_toast(toast);
            }
        ));
        on!(min_height, connect_value_notify, "hwdec_min_height", |r| r.value() as u32);
        on!(optimize, connect_active_notify, "optimize", |r| r.is_active());
        on!(format, connect_selected_notify, "proxy_format", |r| FORMATS.get(r.selected() as usize));
        on!(raw_mb, connect_value_notify, "raw_proxy_max_mb", |r| r.value() as u32);
        on!(cache_mb, connect_value_notify, "proxy_cache_mb", |r| r.value() as u32);
        self.custom.connect_apply(glib::clone!(
            #[weak(rename_to = this)]
            self,
            move |r| this.set("hwdec", json!(r.text().trim()))
        ));
        self.hwdec.connect_selected_notify(glib::clone!(
            #[weak(rename_to = this)]
            self,
            move |r| {
                let other = r.selected() as usize >= HWDEC.len();
                this.custom.set_visible(other);
                if let Some(name) = HWDEC.get(r.selected() as usize) {
                    this.set("hwdec", json!(name));
                }
            }
        ));
        self.login.connect_active_notify(glib::clone!(
            #[weak(rename_to = this)]
            self,
            move |r| {
                if this.updating.get() {
                    return;
                }
                let done = if r.is_active() {
                    sibling_daemon()
                        .ok_or_else(|| std::io::Error::other("aniwayd not found"))
                        .and_then(|d| autostart::enable(&d))
                } else {
                    autostart::disable()
                };
                if let Err(e) = done {
                    this.ctl.toast(&e.to_string());
                }
                this.refresh_login();
            }
        ));
    }

    /// Reads the unit state, and asks systemd (off the main thread) whether this session would start it.
    fn refresh_login(self: &Rc<Self>) {
        self.updating.set(true);
        self.login.set_active(autostart::status().enabled);
        self.updating.set(false);
        self.login.set_subtitle(&tr("Starts the daemon with your graphical session."));
        let this = self.clone();
        glib::spawn_future_local(async move {
            let active = gio::spawn_blocking(|| {
                Command::new("systemctl").args(["--user", "is-active", "graphical-session.target"]).output()
            })
            .await;
            if !matches!(active, Ok(Ok(o)) if o.status.success()) {
                this.login.set_subtitle(&tr("This session does not start graphical-session.target: add aniwayd to your compositor config instead."));
            }
        });
    }

    pub fn present(self: &Rc<Self>, parent: &impl IsA<gtk::Widget>) {
        // The Library view can change the folders too.
        self.settings.replace(settings::load());
        self.rebuild_library();
        self.refresh_login();
        self.updating.set(true);
        let s = self.settings.borrow();
        let chosen = THEMES.iter().position(|t| t.id == s.theme).map_or(0, |i| i + 1);
        self.themes[chosen].set_active(true);
        self.language.set_selected(LANGUAGES.iter().position(|l| *l == s.language).unwrap_or(0) as u32);
        drop(s);
        self.updating.set(false);
        self.update();
        self.dialog.present(Some(parent));
    }

    fn update(&self) {
        let state = self.ctl.state.borrow();
        let Some(c) = state.as_ref().map(|s| &s.config) else { return };
        self.updating.set(true);
        self.tray.set_active(c.tray);
        let known = HWDEC.iter().position(|h| *h == c.hwdec);
        self.hwdec.set_selected(known.unwrap_or(HWDEC.len()) as u32);
        self.custom.set_visible(known.is_none());
        if known.is_none() {
            self.custom.set_text(&c.hwdec);
        }
        self.min_height.set_value(f64::from(c.hwdec_min_height));
        self.optimize.set_active(c.optimize);
        self.format.set_selected(u32::from(c.proxy_format == aniway_proto::model::ProxyFormat::Compressed));
        self.raw_mb.set_value(f64::from(c.raw_proxy_max_mb));
        self.cache_mb.set_value(f64::from(c.proxy_cache_mb));
        self.class_rows.borrow_mut().drain(..).for_each(|r| self.classes.remove(&r));
        for class in &c.hide_for_classes {
            let remove = gtk::Button::builder()
                .icon_name("edit-delete-symbolic")
                .valign(gtk::Align::Center)
                .css_classes(["flat"])
                .tooltip_text(tr("Remove"))
                .build();
            remove.update_property(&[gtk::accessible::Property::Label(&tr("Remove {}").replace("{}", class))]);
            let (ctl, name) = (self.ctl.clone(), class.clone());
            remove.connect_clicked(move |_| {
                let rest: Vec<_> = classes(&ctl).into_iter().filter(|c| *c != name).collect();
                ctl.send(Request::ConfigSet { patch: json!({ "hide_for_classes": rest }) });
            });
            let row = adw::ActionRow::builder().title(glib::markup_escape_text(class)).build();
            row.add_suffix(&remove);
            self.classes.add(&row);
            self.class_rows.borrow_mut().push(row);
        }
        self.updating.set(false);
    }
}

/// Miniature of the editor in `t`: window, list pane, canvas with a dark screen, an accent sticker.
fn swatch(cr: &gtk::cairo::Context, t: &Theme, x: f64, w: f64, h: f64) {
    let fill = |hex: u32, rx: f64, ry: f64, rw: f64, rh: f64| {
        let c = theme::rgb(hex);
        cr.set_source_rgb(c.red().into(), c.green().into(), c.blue().into());
        cr.rectangle(rx, ry, rw, rh);
        let _ = cr.fill();
    };
    let full = 96.0;
    cr.save().ok();
    cr.rectangle(x, 0.0, w, h);
    cr.clip();
    fill(t.window, 0.0, 0.0, full, h);
    fill(t.centre, 28.0, 8.0, full - 28.0, h - 8.0);
    fill(t.screen, 40.0, 18.0, 44.0, 28.0);
    fill(t.accent, 46.0, 24.0, 16.0, 10.0);
    fill(t.accent, 6.0, 12.0, 16.0, 5.0);
    for row in 0..3 {
        fill(if t.dark { 0x3a3d43 } else { 0xd9dadc }, 6.0, 22.0 + 9.0 * f64::from(row), 16.0, 4.0);
    }
    cr.restore().ok();
}

/// Starts a new editor and closes this one.
fn restart() {
    if let Ok(exe) = std::env::current_exe() {
        if Command::new(exe).spawn().is_ok() {
            if let Some(app) = gio::Application::default() {
                app.quit();
            }
        }
    }
}

fn classes(ctl: &Ctl) -> Vec<String> {
    ctl.state.borrow().as_ref().map(|s| s.config.hide_for_classes.clone()).unwrap_or_default()
}
