//! Drop zone at the foot of the overlay list: fanned dashed cards and a "+" invite dropping media, or clicking to
//! browse. It lights up while files are dragged over the window and fills the pane while the scene is empty.

use crate::theme::{self, VINYL};
use crate::tr;
use adw::prelude::*;
use gtk::cairo;
use std::cell::Cell;
use std::f64::consts::PI;
use std::rc::Rc;

pub struct DropZone {
    pub widget: gtk::Button,
    art: gtk::DrawingArea,
    content: gtk::Box,
    title: gtk::Label,
    caption: gtk::Label,
    active: Rc<Cell<bool>>,
    big: Rc<Cell<bool>>,
}

fn rounded(cr: &cairo::Context, x: f64, y: f64, w: f64, h: f64, r: f64) {
    cr.new_sub_path();
    cr.arc(x + w - r, y + r, r, -PI / 2.0, 0.0);
    cr.arc(x + w - r, y + h - r, r, 0.0, PI / 2.0);
    cr.arc(x + r, y + h - r, r, PI / 2.0, PI);
    cr.arc(x + r, y + r, r, PI, 1.5 * PI);
    cr.close_path();
}

fn source(cr: &cairo::Context, hex: u32, alpha: f64) {
    let c = theme::rgb(hex);
    cr.set_source_rgba(c.red().into(), c.green().into(), c.blue().into(), alpha);
}

/// Three dashed cards fanned out behind a solid "+" badge; they spread further while files hover the window.
fn draw(cr: &cairo::Context, width: f64, height: f64, active: bool, big: bool) {
    let t = theme::current();
    let k = if big { 1.5 } else { 1.0 };
    let spread = if active { 1.35 } else { 1.0 };
    let (cw, ch) = (44.0 * k, 32.0 * k);
    let (cx, cy) = (width / 2.0, height / 2.0 + 2.0);
    for (i, (angle, colour)) in [(-15.0f64, VINYL[3]), (15.0, VINYL[2]), (0.0, VINYL[1])].into_iter().enumerate() {
        let dx = [-24.0, 24.0, 0.0][i] * k * spread;
        let dy = if i == 2 { -2.0 * k } else { 4.0 * k };
        cr.save().ok();
        cr.translate(cx + dx, cy + dy);
        cr.rotate((angle * spread).to_radians());
        rounded(cr, -cw / 2.0, -ch / 2.0, cw, ch, 7.0 * k);
        source(cr, colour, if t.dark { 0.16 } else { 0.22 });
        cr.fill_preserve().ok();
        source(cr, colour, 0.95);
        cr.set_line_width(1.6);
        cr.set_dash(&[4.0, 3.0], 0.0);
        cr.stroke().ok();
        cr.restore().ok();
    }
    let r = 11.0 * k;
    cr.arc(cx, cy - 2.0 * k, r, 0.0, 2.0 * PI);
    source(cr, t.accent, 1.0);
    cr.fill().ok();
    source(cr, t.accent_fg, 1.0);
    cr.set_line_width(2.2 * k);
    cr.set_line_cap(cairo::LineCap::Round);
    let (px, py, arm) = (cx, cy - 2.0 * k, r * 0.45);
    cr.move_to(px - arm, py);
    cr.line_to(px + arm, py);
    cr.move_to(px, py - arm);
    cr.line_to(px, py + arm);
    cr.stroke().ok();
}

impl DropZone {
    pub fn new() -> Rc<Self> {
        let art = gtk::DrawingArea::builder().content_height(64).build();
        let title = gtk::Label::builder().css_classes(["title"]).wrap(true).justify(gtk::Justification::Center).build();
        let caption =
            gtk::Label::builder().css_classes(["caption"]).wrap(true).justify(gtk::Justification::Center).build();
        let content = gtk::Box::new(gtk::Orientation::Vertical, 2);
        content.append(&art);
        content.append(&title);
        content.append(&caption);
        let widget = gtk::Button::builder()
            .child(&content)
            .action_name("win.add")
            .css_classes(["dropzone"])
            .margin_start(12)
            .margin_end(12)
            .margin_top(6)
            .margin_bottom(8)
            .tooltip_text(tr("Add media"))
            .build();
        let this = Rc::new(Self { widget, art, content, title, caption, active: Rc::default(), big: Rc::default() });
        let (active, big) = (this.active.clone(), this.big.clone());
        this.art.set_draw_func(move |_, cr, w, h| draw(cr, f64::from(w), f64::from(h), active.get(), big.get()));
        let art = this.art.downgrade();
        theme::on_change(move || art.upgrade().map_or((), |a| a.queue_draw()));
        this.set_empty(false, "");
        this
    }

    /// Highlights the zone while files are dragged over the window.
    pub fn set_active(&self, active: bool) {
        self.active.set(active);
        if active {
            self.widget.add_css_class("active");
        } else {
            self.widget.remove_css_class("active");
        }
        self.art.queue_draw();
    }

    /// An empty scene turns the zone into the pane's whole content, naming the monitor that receives the media.
    pub fn set_empty(&self, empty: bool, output: &str) {
        self.big.set(empty);
        self.widget.set_vexpand(empty);
        // Alone in the pane the invitation sits in the middle; under a list it stays compact at the foot.
        self.content.set_valign(if empty { gtk::Align::Center } else { gtk::Align::Fill });
        self.art.set_content_height(if empty { 110 } else { 64 });
        if empty {
            self.title.set_text(&tr("Drop media to add it to {}").replace("{}", output));
            self.caption.set_text(&tr("Videos, GIFs and images become overlays. Or click to browse."));
        } else {
            self.title.set_text(&tr("Drop media here"));
            self.caption.set_text(&tr("or click to browse · several at once"));
        }
        self.art.queue_draw();
    }
}
