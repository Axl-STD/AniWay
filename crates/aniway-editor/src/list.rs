//! Overlay list (left pane): one row per overlay with its sticker number, monitor, format and length, a show/hide
//! button, a delete button that asks first, and the drop zone pinned underneath so media can be added however long the list is.

use crate::client::Ctl;
use crate::dropzone::DropZone;
use crate::map::{display_number, file_name, vinyl_index};
use crate::media::Media;
use crate::{ngettext, tr};
use adw::prelude::*;
use aniway_proto::ipc::{OverlayInfo, Request, StateData};
use aniway_proto::model::OverlayPatch;
use aniway_proto::paths;
use gtk::gio;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

struct Row {
    num: gtk::Label,
    name: gtk::Label,
    output: gtk::Label,
    format: gtk::Label,
    duration: (gtk::Box, gtk::Label),
    state: gtk::Label,
    thumb: gtk::Image,
    eye: gtk::Button,
}

pub struct List {
    pub widget: gtk::Box,
    pub drop: Rc<DropZone>,
    scroller: gtk::ScrolledWindow,
    list: gtk::ListBox,
    rows: RefCell<Vec<(String, Row)>>,
    hw: gtk::Label,
    count: gtk::Label,
    updating: Cell<bool>,
    ctl: Rc<Ctl>,
}

fn label(css: &[&str]) -> gtk::Label {
    gtk::Label::builder().xalign(0.0).ellipsize(gtk::pango::EllipsizeMode::End).css_classes(css).build()
}

/// Icon and text of one property of a row.
fn meta(icon: &str, tooltip: &str) -> (gtk::Box, gtk::Label) {
    let (b, l) = (gtk::Box::new(gtk::Orientation::Horizontal, 5), gtk::Label::new(None));
    b.append(&gtk::Image::builder().icon_name(icon).pixel_size(14).build());
    b.append(&l);
    b.set_tooltip_text(Some(tooltip));
    (b, l)
}

/// Asks before removing an overlay; the toast that follows still offers to undo it.
pub fn confirm_remove(parent: &impl IsA<gtk::Widget>, ctl: &Rc<Ctl>, id: &str) {
    let found = ctl.state.borrow().as_ref().and_then(|s| s.overlays.iter().find(|o| o.config.id == id).cloned());
    let Some(overlay) = found else { return };
    let dialog = adw::AlertDialog::new(
        Some(&tr("Remove {}?").replace("{}", file_name(&overlay.config.path))),
        Some(&tr("It leaves the screen and this scene. The file itself is kept.")),
    );
    dialog.add_responses(&[("cancel", &tr("Cancel")), ("remove", &tr("Remove"))]);
    dialog.set_response_appearance("remove", adw::ResponseAppearance::Destructive);
    dialog.set_close_response("cancel");
    let ctl = ctl.clone();
    dialog.choose(Some(parent), gio::Cancellable::NONE, move |r| {
        if r == "remove" {
            ctl.remove(overlay.config);
        }
    });
}

fn build_row(ctl: &Rc<Ctl>, id: &str) -> (gtk::ListBoxRow, Row) {
    let output = meta("video-single-display-symbolic", &tr("Monitor"));
    let format = meta("aniway-format-symbolic", &tr("Format"));
    let duration = meta("preferences-system-time-symbolic", &tr("Length"));
    let row = Row {
        num: gtk::Label::builder().css_classes(["vinyl"]).valign(gtk::Align::Center).build(),
        name: label(&["row-title"]),
        output: output.1,
        format: format.1,
        duration: duration.clone(),
        state: gtk::Label::builder().xalign(0.0).wrap(true).css_classes(["state"]).build(),
        thumb: gtk::Image::builder()
            .valign(gtk::Align::Center)
            .css_classes(["thumb"])
            .pixel_size(52)
            .overflow(gtk::Overflow::Hidden)
            .build(),
        eye: gtk::Button::builder().valign(gtk::Align::Center).css_classes(["flat", "circular", "row-action"]).build(),
    };
    let title = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    title.append(&row.num);
    title.append(&row.name);
    let info = gtk::Box::builder().spacing(12).css_classes(["meta"]).build();
    for b in [&output.0, &format.0, &duration.0] {
        info.append(b);
    }
    let text = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(4).hexpand(true).build();
    text.set_valign(gtk::Align::Center);
    for w in [title.upcast_ref::<gtk::Widget>(), info.upcast_ref(), row.state.upcast_ref()] {
        text.append(w);
    }
    let (c, id_for_eye) = (ctl.clone(), id.to_string());
    row.eye.connect_clicked(move |_| {
        let visible = c
            .state
            .borrow()
            .as_ref()
            .and_then(|s| s.overlays.iter().find(|o| o.config.id == id_for_eye).map(|o| o.config.visible));
        if let Some(visible) = visible {
            let patch = OverlayPatch { visible: Some(!visible), ..Default::default() };
            c.send(Request::Set { id: id_for_eye.clone(), patch, transient: false });
        }
    });
    let delete = gtk::Button::builder()
        .icon_name("user-trash-symbolic")
        .valign(gtk::Align::Center)
        .css_classes(["flat", "circular", "row-action"])
        .tooltip_text(tr("Remove…"))
        .build();
    let (c, id) = (ctl.clone(), id.to_string());
    delete.connect_clicked(move |b| confirm_remove(b, &c, &id));
    let outer = gtk::Box::builder().spacing(12).margin_top(10).margin_bottom(10).margin_start(10).margin_end(6).build();
    outer.append(&row.thumb);
    outer.append(&text);
    outer.append(&row.eye);
    outer.append(&delete);
    (gtk::ListBoxRow::builder().child(&outer).build(), row)
}

fn state_line(state: &StateData, ov: &OverlayInfo) -> Option<(String, bool)> {
    let output = ov.config.output.as_deref().or(state.outputs.first().map(|o| o.name.as_str()));
    if let Some(error) = &ov.error {
        Some((error.clone(), true))
    } else if output.is_some_and(|n| !state.outputs.iter().any(|o| o.name == n)) {
        Some((tr("Monitor disconnected"), false))
    } else if !ov.config.visible {
        Some((tr("Hidden"), false))
    } else if state.hidden || state.snooze_secs_left.is_some() {
        Some((tr("All overlays are hidden"), false))
    } else if !ov.shown {
        Some((tr("Hidden by an app rule"), false))
    } else if state.paused && ov.config.playing {
        Some((tr("All overlays are paused"), false))
    } else {
        None
    }
}

impl Row {
    fn set(&self, index: usize, state: &StateData, ov: &OverlayInfo, media: Option<&Media>) {
        let c = &ov.config;
        self.num.set_text(&display_number(index));
        self.num.set_css_classes(&["vinyl", &format!("vinyl-{}", vinyl_index(index))]);
        self.name.set_text(file_name(&c.path));
        self.name.set_tooltip_text(Some(&c.path));
        self.output.set_text(c.output.as_deref().or(state.outputs.first().map(|o| o.name.as_str())).unwrap_or(""));
        let ext = paths::extension(&c.path).to_uppercase();
        self.format.set_text(if ext.is_empty() { "—" } else { &ext });
        let length = media.map_or(String::new(), Media::duration_text);
        self.duration.0.set_visible(!length.is_empty());
        self.duration.1.set_text(&length);
        match media.and_then(|m| m.texture.as_ref()) {
            Some(t) => self.thumb.set_paintable(Some(t)),
            None => self.thumb.set_icon_name(Some("video-x-generic-symbolic")),
        }
        // A hidden overlay keeps its eye fully visible, so the row shows at a glance that it is off.
        self.eye.set_icon_name(if c.visible { "view-reveal-symbolic" } else { "view-conceal-symbolic" });
        self.eye.set_tooltip_text(Some(&if c.visible { tr("Hide") } else { tr("Show") }));
        if c.visible {
            self.eye.remove_css_class("off");
        } else {
            self.eye.add_css_class("off");
        }
        let line = state_line(state, ov);
        self.state.set_visible(line.is_some());
        self.state.set_text(line.as_ref().map_or("", |l| &l.0));
        if line.is_some_and(|l| l.1) {
            self.state.add_css_class("error");
        } else {
            self.state.remove_css_class("error");
        }
    }
}

impl List {
    pub fn new(ctl: Rc<Ctl>) -> Rc<Self> {
        let list =
            gtk::ListBox::builder().selection_mode(gtk::SelectionMode::Single).css_classes(["overlay-list"]).build();
        list.update_property(&[gtk::accessible::Property::Label(&tr("Overlays"))]);
        let scroller =
            gtk::ScrolledWindow::builder().child(&list).vexpand(true).hscrollbar_policy(gtk::PolicyType::Never).build();
        let footer = gtk::Box::builder()
            .spacing(12)
            .margin_bottom(8)
            .margin_start(16)
            .margin_end(16)
            .css_classes(["list-footer"])
            .build();
        let (hw, count) = (label(&["cols"]), gtk::Label::builder().css_classes(["cols"]).build());
        hw.set_hexpand(true);
        footer.append(&hw);
        footer.append(&count);
        let drop = DropZone::new();
        let widget = gtk::Box::new(gtk::Orientation::Vertical, 0);
        widget.set_width_request(260);
        widget.append(&scroller);
        widget.append(&drop.widget);
        widget.append(&footer);
        let this = Rc::new(Self {
            widget,
            drop,
            scroller,
            list,
            rows: RefCell::default(),
            hw,
            count,
            updating: Cell::default(),
            ctl,
        });
        let handler = this.clone();
        this.list.connect_row_selected(move |_, row| {
            if !handler.updating.get() {
                let id = row.and_then(|r| handler.rows.borrow().get(r.index() as usize).map(|(id, _)| id.clone()));
                handler.ctl.select(id);
            }
        });
        this
    }

    pub fn update(&self) {
        let state = self.ctl.state.borrow();
        let Some(state) = state.as_ref() else { return };
        self.updating.set(true);
        let same = state.overlays.len() == self.rows.borrow().len()
            && state.overlays.iter().zip(self.rows.borrow().iter()).all(|(o, (id, _))| &o.config.id == id);
        if !same {
            self.list.remove_all();
            let mut rows = self.rows.borrow_mut();
            rows.clear();
            for ov in &state.overlays {
                let (widget, row) = build_row(&self.ctl, &ov.config.id);
                self.list.append(&widget);
                rows.push((ov.config.id.clone(), row));
            }
        }
        for (i, (ov, (_, row))) in state.overlays.iter().zip(self.rows.borrow().iter()).enumerate() {
            row.set(i, state, ov, self.ctl.media.borrow().get(&ov.config.path));
        }
        let selected = self.ctl.selected.borrow();
        let index = selected.as_ref().and_then(|id| state.overlays.iter().position(|o| &o.config.id == id));
        self.list.select_row(index.and_then(|i| self.list.row_at_index(i as i32)).as_ref());
        let empty = state.overlays.is_empty();
        self.scroller.set_visible(!empty);
        self.drop.set_empty(empty, self.ctl.focus.borrow().as_deref().unwrap_or(""));
        let mut decoders: Vec<&str> =
            state.overlays.iter().filter_map(|o| o.hwdec.as_deref()).filter(|d| *d != "no").collect();
        decoders.sort_unstable();
        decoders.dedup();
        self.hw.set_text(&if decoders.is_empty() { tr("software decoding") } else { decoders.join(", ") });
        let n = state.overlays.len();
        self.count.set_text(&ngettext("{} overlay", "{} overlays", n as u32).replace("{}", &n.to_string()));
        self.updating.set(false);
    }
}
