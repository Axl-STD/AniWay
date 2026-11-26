//! Scene menu of the header bar: switch, create, edit (name and an optional Hyprland shortcut) and delete scenes.

use crate::client::{self, Ctl};
use crate::tr;
use adw::prelude::*;
use aniway_proto::ipc::Request;
use aniway_proto::paths::valid_scene_name;
use gtk::{gdk, gio, glib};
use serde_json::json;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

const MODIFIER_KEYS: [&str; 13] = [
    "Shift_L",
    "Shift_R",
    "Control_L",
    "Control_R",
    "Alt_L",
    "Alt_R",
    "Super_L",
    "Super_R",
    "Meta_L",
    "Meta_R",
    "Hyper_L",
    "Hyper_R",
    "ISO_Level3_Shift",
];

/// Hyprland's "SUPER + SHIFT + F1" as the GTK accelerator "<Super><Shift>F1", for display.
fn accel(keys: &str) -> String {
    let mut parts: Vec<&str> = keys.split('+').map(str::trim).collect();
    let key = parts.pop().unwrap_or_default();
    let mods: String = parts
        .iter()
        .map(|m| match m.to_ascii_uppercase().as_str() {
            "SUPER" => "<Super>",
            "CTRL" => "<Control>",
            "ALT" => "<Alt>",
            _ => "<Shift>",
        })
        .collect();
    format!("{mods}{key}")
}

/// Localized, compact label of Hyprland keys ("Mayús+Super+F1").
fn keys_label(keys: &str) -> String {
    gtk::accelerator_parse(accel(keys)).map_or(keys.to_string(), |(k, m)| gtk::accelerator_get_label(k, m).into())
}

/// A key press in Hyprland's notation. A lone modifier gives nothing, and so does a plain key without modifiers
/// (it would steal typing in every app), function keys aside.
fn keys_of(key: gdk::Key, mods: gdk::ModifierType) -> Option<String> {
    let name = key.name()?;
    if MODIFIER_KEYS.contains(&name.as_str()) {
        return None;
    }
    let name = key.to_lower().name()?.to_string();
    let mut parts: Vec<String> = [
        (gdk::ModifierType::SUPER_MASK | gdk::ModifierType::META_MASK, "SUPER"),
        (gdk::ModifierType::CONTROL_MASK, "CTRL"),
        (gdk::ModifierType::ALT_MASK, "ALT"),
        (gdk::ModifierType::SHIFT_MASK, "SHIFT"),
    ]
    .into_iter()
    .filter(|(mask, _)| mods.intersects(*mask))
    .map(|(_, m)| m.to_string())
    .collect();
    let function = name.strip_prefix('F').is_some_and(|n| n.parse::<u8>().is_ok());
    if parts.is_empty() && !function {
        return None;
    }
    parts.push(name);
    Some(parts.join(" + "))
}

/// Shortcut of every scene, as the daemon last reported.
fn scene_keys(ctl: &Ctl) -> BTreeMap<String, String> {
    ctl.state.borrow().as_ref().map(|s| s.config.scene_keys.clone()).unwrap_or_default()
}

/// Dialog that names a scene and optionally records its shortcut; `done` gets the trimmed name and the keys.
fn scene_dialog(
    parent: &impl IsA<gtk::Widget>,
    heading: &str,
    ok: &str,
    (name, keys): (&str, Option<String>),
    done: impl Fn(String, Option<String>) + 'static,
) {
    let entry = adw::EntryRow::builder().title(tr("Name")).text(name).activates_default(true).build();
    let label = gtk::Label::builder().valign(gtk::Align::Center).css_classes(["keycap"]).build();
    let record = gtk::ToggleButton::builder()
        .icon_name("input-keyboard-symbolic")
        .valign(gtk::Align::Center)
        .css_classes(["circular"])
        .tooltip_text(tr("Record a shortcut"))
        .build();
    let clear = gtk::Button::builder()
        .icon_name("edit-clear-symbolic")
        .valign(gtk::Align::Center)
        .css_classes(["flat", "circular"])
        .tooltip_text(tr("Remove the shortcut"))
        .build();
    let shortcut = adw::ActionRow::builder().title(tr("Shortcut")).build();
    shortcut.add_suffix(&label);
    shortcut.add_suffix(&clear);
    shortcut.add_suffix(&record);
    let rows = gtk::ListBox::builder().css_classes(["boxed-list"]).selection_mode(gtk::SelectionMode::None).build();
    rows.append(&entry);
    rows.append(&shortcut);
    let note = gtk::Label::builder()
        .label(tr("Optional: switches to this scene from anywhere, even with the editor closed."))
        .wrap(true)
        .xalign(0.0)
        .css_classes(["caption", "dim-label"])
        .build();
    let extra = gtk::Box::new(gtk::Orientation::Vertical, 8);
    extra.append(&rows);
    extra.append(&note);
    let dialog = adw::AlertDialog::builder().heading(heading).extra_child(&extra).prefer_wide_layout(true).build();
    dialog.add_responses(&[("cancel", &tr("Cancel")), ("ok", ok)]);
    dialog.set_response_appearance("ok", adw::ResponseAppearance::Suggested);
    dialog.set_default_response(Some("ok"));
    dialog.set_close_response("cancel");

    let keys = Rc::new(RefCell::new(keys));
    let show = {
        let (keys, label, clear) = (keys.clone(), label.clone(), clear.clone());
        move || {
            label.set_text(&keys.borrow().as_deref().map_or(tr("None"), keys_label));
            clear.set_visible(keys.borrow().is_some());
        }
    };
    show();
    let show = Rc::new(show);
    // xgettext skips strings inside macro calls.
    let waiting = tr("Press the keys…");
    record.connect_toggled(glib::clone!(
        #[weak]
        label,
        #[strong]
        show,
        move |b| {
            if b.is_active() {
                label.set_text(&waiting);
            } else {
                show();
            }
        }
    ));
    clear.connect_clicked(glib::clone!(
        #[strong]
        keys,
        #[strong]
        show,
        move |_| {
            keys.replace(None);
            show();
        }
    ));
    let capture = gtk::EventControllerKey::new();
    capture.set_propagation_phase(gtk::PropagationPhase::Capture);
    capture.connect_key_pressed(glib::clone!(
        #[strong]
        keys,
        #[strong]
        show,
        #[weak]
        record,
        #[upgrade_or]
        glib::Propagation::Proceed,
        move |controller, key, code, mods| {
            if !record.is_active() {
                return glib::Propagation::Proceed;
            }
            // SHIFT changes the keysym ("1" becomes "exclam"); record the unshifted key so SUPER + SHIFT + 1 works.
            let key = controller
                .widget()
                .map(|w| w.display())
                .and_then(|d| d.translate_key(code, gdk::ModifierType::empty(), 0))
                .map_or(key, |(base, ..)| base);
            if key == gdk::Key::Escape {
                record.set_active(false);
            } else if let Some(k) = keys_of(key, mods) {
                keys.replace(Some(k));
                show();
                record.set_active(false);
            }
            glib::Propagation::Stop
        }
    ));
    dialog.add_controller(capture);
    let valid = |e: &adw::EntryRow| valid_scene_name(e.text().trim());
    dialog.set_response_enabled("ok", valid(&entry));
    entry.connect_changed(glib::clone!(
        #[weak]
        dialog,
        move |e| dialog.set_response_enabled("ok", valid(e))
    ));
    dialog.choose(Some(parent), gio::Cancellable::NONE, move |response| {
        if response == "ok" && valid(&entry) {
            done(entry.text().trim().to_string(), keys.borrow().clone());
        }
    });
}

/// Requests in order, stopping at the first failure (shown as a toast).
fn run(ctl: &Rc<Ctl>, requests: Vec<Request>) {
    let ctl = ctl.clone();
    glib::spawn_future_local(async move {
        for req in requests {
            if let Err(e) = client::call(req).await {
                return ctl.toast(&e.to_string());
            }
        }
    });
}

/// `scene_keys` with the shortcut of `old` moved to `name` (or dropped), as a `config_set` request.
fn keys_request(ctl: &Ctl, old: Option<&str>, name: &str, keys: Option<String>) -> Request {
    let mut all = scene_keys(ctl);
    if let Some(old) = old {
        all.remove(old);
    }
    all.remove(name);
    if let Some(keys) = keys {
        all.insert(name.to_string(), keys);
    }
    Request::ConfigSet { patch: json!({ "scene_keys": all }) }
}

/// Creates an empty scene and switches to it; the active scene keeps what it has.
pub fn new_scene(ctl: &Rc<Ctl>, parent: &impl IsA<gtk::Widget>) {
    let ctl = ctl.clone();
    scene_dialog(parent, &tr("New scene"), &tr("Create"), ("", None), move |name, keys| {
        let mut requests = vec![Request::SceneNew { name: name.clone() }];
        if keys.is_some() {
            requests.push(keys_request(&ctl, None, &name, keys));
        }
        run(&ctl, requests);
    });
}

/// Copies the active scene under a new name and switches to the copy.
fn duplicate_scene(ctl: &Rc<Ctl>, parent: &impl IsA<gtk::Widget>) {
    let ctl = ctl.clone();
    scene_dialog(parent, &tr("Duplicate scene"), &tr("Duplicate"), ("", None), move |name, keys| {
        let mut requests = vec![Request::SceneSave { name: name.clone() }];
        if keys.is_some() {
            requests.push(keys_request(&ctl, None, &name, keys));
        }
        requests.push(Request::SceneLoad { name });
        run(&ctl, requests);
    });
}

fn edit_scene(ctl: &Rc<Ctl>, parent: &impl IsA<gtk::Widget>, old: String) {
    let ctl = ctl.clone();
    let current = scene_keys(&ctl).get(&old).cloned();
    let heading = tr("Edit scene {}").replace("{}", &old);
    let shown = old.clone();
    scene_dialog(parent, &heading, &tr("Save"), (&shown, current.clone()), move |name, keys| {
        let mut requests = Vec::new();
        if name != old {
            requests.push(Request::SceneRename { name: old.clone(), to: name.clone() });
        }
        if keys != current || name != old {
            requests.push(keys_request(&ctl, Some(&old), &name, keys.clone()));
        }
        run(&ctl, requests);
    });
}

fn delete_scene(ctl: &Rc<Ctl>, parent: &impl IsA<gtk::Widget>, name: String) {
    let dialog = adw::AlertDialog::new(
        Some(&tr("Delete scene {}?").replace("{}", &name)),
        Some(&tr("The scene file is removed from disk, with its shortcut.")),
    );
    dialog.add_responses(&[("cancel", &tr("Cancel")), ("delete", &tr("Delete"))]);
    dialog.set_response_appearance("delete", adw::ResponseAppearance::Destructive);
    dialog.set_close_response("cancel");
    let ctl = ctl.clone();
    dialog.choose(Some(parent), gio::Cancellable::NONE, move |r| {
        if r == "delete" {
            ctl.send(Request::SceneDelete { name });
        }
    });
}

fn icon_button(icon: &str, tooltip: &str) -> gtk::Button {
    let b = gtk::Button::builder()
        .icon_name(icon)
        .css_classes(["flat", "circular"])
        .valign(gtk::Align::Center)
        .tooltip_text(tooltip)
        .build();
    b.update_property(&[gtk::accessible::Property::Label(tooltip)]);
    b
}

pub struct SceneMenu {
    pub button: gtk::MenuButton,
    label: gtk::Label,
}

impl SceneMenu {
    pub fn new(ctl: &Rc<Ctl>, window: &adw::ApplicationWindow) -> Self {
        let list = gtk::Box::new(gtk::Orientation::Vertical, 2);
        let action = |icon: &str, label: String| {
            let content = adw::ButtonContent::builder().icon_name(icon).label(label).halign(gtk::Align::Start).build();
            gtk::Button::builder().child(&content).css_classes(["flat"]).build()
        };
        let create = action("list-add-symbolic", tr("New empty scene…"));
        let duplicate = action("edit-copy-symbolic", tr("Duplicate this scene…"));
        let hint = gtk::Label::builder()
            .label(tr("Changes save to the active scene as you make them."))
            .xalign(0.0)
            .wrap(true)
            .css_classes(["caption", "dim-label"])
            .margin_start(8)
            .margin_end(8)
            .margin_bottom(4)
            .build();
        let heading = gtk::Label::builder().label(tr("Scenes")).xalign(0.0).css_classes(["heading"]).build();
        heading.set_margin_start(8);
        heading.set_margin_bottom(4);
        let content = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(6).width_request(300).build();
        content.append(&heading);
        content.append(&hint);
        content.append(&list);
        content.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        content.append(&create);
        content.append(&duplicate);
        let popover = gtk::Popover::builder().child(&content).css_classes(["scenes"]).build();
        let label = gtk::Label::builder().ellipsize(gtk::pango::EllipsizeMode::End).max_width_chars(16).build();
        let inner = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        inner.append(&gtk::Image::from_icon_name("aniway-scenes-symbolic"));
        inner.append(&label);
        let button = gtk::MenuButton::builder().popover(&popover).child(&inner).always_show_arrow(true).build();
        button.set_tooltip_text(Some(&tr("Scenes")));
        // xgettext skips strings inside macro calls.
        let (edit_tip, delete_tip) = (tr("Rename or set a shortcut…"), tr("Delete scene…"));
        create.connect_clicked(glib::clone!(
            #[strong]
            ctl,
            #[weak]
            window,
            #[weak]
            popover,
            move |_| {
                popover.popdown();
                new_scene(&ctl, &window);
            }
        ));
        duplicate.connect_clicked(glib::clone!(
            #[strong]
            ctl,
            #[weak]
            window,
            #[weak]
            popover,
            move |_| {
                popover.popdown();
                duplicate_scene(&ctl, &window);
            }
        ));
        popover.connect_show(glib::clone!(
            #[strong]
            ctl,
            #[weak]
            window,
            #[weak]
            popover,
            move |_| {
                let (ctl, list, edit_tip, delete_tip) =
                    (ctl.clone(), list.clone(), edit_tip.clone(), delete_tip.clone());
                glib::spawn_future_local(async move {
                    let Ok(data) = client::call(Request::SceneList).await else { return };
                    let active = data["active"].as_str().unwrap_or_default().to_string();
                    let keys = scene_keys(&ctl);
                    while let Some(row) = list.first_child() {
                        list.remove(&row);
                    }
                    let mut leader: Option<gtk::CheckButton> = None;
                    for name in data["scenes"].as_array().into_iter().flatten().filter_map(|n| n.as_str()) {
                        let check = gtk::CheckButton::builder().label(name).hexpand(true).build();
                        check.set_group(leader.as_ref());
                        check.set_active(name == active);
                        leader.get_or_insert_with(|| check.clone());
                        let (c, n, p) = (ctl.clone(), name.to_string(), popover.clone());
                        let current = active.clone();
                        check.connect_toggled(move |b| {
                            if b.is_active() && n != current {
                                c.send(Request::SceneLoad { name: n.clone() });
                                p.popdown();
                            }
                        });
                        let row = gtk::Box::builder().spacing(4).css_classes(["scene-row"]).build();
                        row.append(&check);
                        if let Some(k) = keys.get(name) {
                            row.append(
                                &gtk::Label::builder()
                                    .label(keys_label(k))
                                    .css_classes(["keycap", "dim-label"])
                                    .build(),
                            );
                        }
                        let edit = icon_button("document-edit-symbolic", &edit_tip);
                        let (c, n, p, w) = (ctl.clone(), name.to_string(), popover.clone(), window.clone());
                        edit.connect_clicked(move |_| {
                            p.popdown();
                            edit_scene(&c, &w, n.clone());
                        });
                        let delete = icon_button("user-trash-symbolic", &delete_tip);
                        delete.set_sensitive(name != active);
                        let (c, n, p, w) = (ctl.clone(), name.to_string(), popover.clone(), window.clone());
                        delete.connect_clicked(move |_| {
                            p.popdown();
                            delete_scene(&c, &w, n.clone());
                        });
                        row.append(&edit);
                        row.append(&delete);
                        list.append(&row);
                    }
                });
            }
        ));
        Self { button, label }
    }

    pub fn set_scene(&self, name: &str) {
        self.label.set_text(name);
        self.button.set_tooltip_text(Some(&tr("Scene: {}").replace("{}", name)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shortcuts_convert() {
        assert_eq!(accel("SUPER + SHIFT + F1"), "<Super><Shift>F1");
        let both = gdk::ModifierType::SUPER_MASK | gdk::ModifierType::SHIFT_MASK;
        assert_eq!(keys_of(gdk::Key::A, both).as_deref(), Some("SUPER + SHIFT + a"));
        assert_eq!(keys_of(gdk::Key::F5, gdk::ModifierType::empty()).as_deref(), Some("F5"));
        assert_eq!(keys_of(gdk::Key::a, gdk::ModifierType::empty()), None);
        assert_eq!(keys_of(gdk::Key::Shift_L, gdk::ModifierType::SHIFT_MASK), None);
    }
}
