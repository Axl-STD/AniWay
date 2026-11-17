//! Inspector (right pane): every overlay property in native preference rows. Widgets are filled from the
//! last state under the `updating` guard, so only user input produces `set` requests.

use crate::client::Ctl;
use crate::map::file_name;
use crate::media::{self, Media, PREVIEW_FPS};
use crate::{ngettext, tr};
use adw::prelude::*;
use aniway_proto::geometry::{offset_for, place, Point, Size};
use aniway_proto::ipc::{OverlayInfo, ProxyState, Request, StateData};
use aniway_proto::model::{Anchor, Decode, Layer, Overlay, OverlayPatch};
use aniway_proto::paths;
use gtk::glib;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

const ANCHORS: [Anchor; 9] = {
    use Anchor::*;
    [TopLeft, Top, TopRight, Left, Center, Right, BottomLeft, Bottom, BottomRight]
};
const LAYERS: [Layer; 4] = [Layer::Overlay, Layer::Top, Layer::Bottom, Layer::Background];
const DECODE: [Decode; 3] = [Decode::Auto, Decode::Hardware, Decode::Software];

pub struct Inspector {
    pub widget: gtk::Stack,
    ctl: Rc<Ctl>,
    updating: Cell<bool>,
    preview: gtk::Picture,
    preview_toggle: gtk::Button,
    /// File shown by the preview; empty until the probe said whether it moves.
    preview_path: RefCell<String>,
    /// Frames of the moving preview, and the timer that flips them.
    preview_frames: RefCell<Rc<Vec<gtk::gdk::Texture>>>,
    preview_timer: RefCell<Option<glib::SourceId>>,
    /// Whether the preview should play when it is on screen.
    preview_on: Cell<bool>,
    title: gtk::Label,
    subtitle: gtk::Label,
    details: gtk::Label,
    banner: adw::Banner,
    none: adw::StatusPage,
    outputs: RefCell<Vec<String>>,
    output: adw::ComboRow,
    anchors: Vec<gtk::ToggleButton>,
    x: adw::SpinRow,
    y: adw::SpinRow,
    scale: gtk::Scale,
    exact: adw::ExpanderRow,
    width: adw::SpinRow,
    height: adw::SpinRow,
    opacity: gtk::Scale,
    speed: adw::SpinRow,
    layer: adw::ComboRow,
    looping: adw::SwitchRow,
    volume: gtk::Scale,
    visible: adw::SwitchRow,
    playing: adw::SwitchRow,
    fps: adw::ExpanderRow,
    fps_value: adw::SpinRow,
    decode: adw::ComboRow,
    optimize: adw::SwitchRow,
    overflow: adw::SwitchRow,
}

pub fn spin(title: &str, min: f64, max: f64, step: f64, digits: u32) -> adw::SpinRow {
    let row = adw::SpinRow::with_range(min, max, step);
    row.set_title(&tr(title));
    row.set_digits(digits);
    row
}

pub fn switch(title: &str) -> adw::SwitchRow {
    adw::SwitchRow::builder().title(tr(title)).build()
}

pub fn combo(title: &str, items: &[&str]) -> adw::ComboRow {
    adw::ComboRow::builder().title(tr(title)).model(&gtk::StringList::new(items)).build()
}

fn expander(title: &str) -> adw::ExpanderRow {
    adw::ExpanderRow::builder().title(tr(title)).show_enable_switch(true).build()
}

/// Smallest scale the slider reaches.
const MIN_SCALE: f64 = 0.02;

/// Scale at which the media fills its monitor in one dimension: the daemon shrinks anything larger to fit, so the
/// slider ends there and every step of it changes the size.
fn fit_scale(state: &StateData, ov: &OverlayInfo, media: Option<Size>) -> f64 {
    let name = ov.config.output.as_deref();
    let output = state.outputs.iter().find(|o| Some(o.name.as_str()) == name).or(state.outputs.first());
    match (output, media) {
        (Some(o), Some(m)) if m.w > 0 && m.h > 0 => {
            (f64::from(o.rect.w) / f64::from(m.w)).min(f64::from(o.rect.h) / f64::from(m.h))
        }
        _ => 4.0,
    }
}

fn slider(min: f64, max: f64, step: f64) -> gtk::Scale {
    let scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, min, max, step);
    scale.set_width_request(120);
    scale.set_valign(gtk::Align::Center);
    scale
}

/// Row with a slider and a fixed-width value readout.
fn slider_row(title: &str, scale: &gtk::Scale, format: fn(f64) -> String) -> adw::ActionRow {
    let readout = gtk::Label::builder().width_chars(7).xalign(1.0).css_classes(["cols"]).build();
    let target = readout.clone();
    let update = move |s: &gtk::Scale| target.set_text(&format(s.value()));
    update(scale);
    scale.connect_value_changed(update);
    let row = adw::ActionRow::builder().title(tr(title)).build();
    row.add_suffix(scale);
    row.add_suffix(&readout);
    row
}

/// Preference group under a heading with an icon.
fn group(title: &str, icon: &str, rows: &[&gtk::Widget]) -> gtk::Box {
    let heading = gtk::Box::builder().spacing(8).margin_start(2).build();
    heading.append(&gtk::Image::builder().icon_name(icon).css_classes(["section-icon"]).build());
    heading.append(&gtk::Label::builder().label(tr(title)).css_classes(["section-title"]).xalign(0.0).build());
    let g = adw::PreferencesGroup::new();
    rows.iter().for_each(|r| g.add(*r));
    let b = gtk::Box::new(gtk::Orientation::Vertical, 8);
    b.append(&heading);
    b.append(&g);
    b
}

fn text(css: &str) -> gtk::Label {
    gtk::Label::builder().xalign(0.0).css_classes([css]).ellipsize(gtk::pango::EllipsizeMode::Middle).build()
}

impl Inspector {
    pub fn new(ctl: Rc<Ctl>) -> Rc<Self> {
        let this = Rc::new(Self {
            widget: gtk::Stack::new(),
            ctl,
            updating: Cell::default(),
            preview: gtk::Picture::builder()
                .content_fit(gtk::ContentFit::Contain)
                .height_request(170)
                .css_classes(["preview"])
                .overflow(gtk::Overflow::Hidden)
                .build(),
            preview_toggle: gtk::Button::builder()
                .icon_name("media-playback-pause-symbolic")
                .halign(gtk::Align::End)
                .valign(gtk::Align::End)
                .css_classes(["circular", "preview-toggle"])
                .tooltip_text(tr("Pause the preview"))
                .build(),
            preview_path: RefCell::default(),
            preview_frames: RefCell::default(),
            preview_timer: RefCell::default(),
            preview_on: Cell::new(true),
            title: text("inspector-name"),
            subtitle: text("caption"),
            details: gtk::Label::builder().xalign(0.0).wrap(true).css_classes(["cols"]).build(),
            banner: adw::Banner::builder().revealed(false).build(),
            none: adw::StatusPage::builder().css_classes(["compact"]).build(),
            outputs: RefCell::default(),
            output: combo("Monitor", &[]),
            anchors: (0..9)
                .map(|_| {
                    gtk::ToggleButton::builder().width_request(26).height_request(26).css_classes(["anchor"]).build()
                })
                .collect(),
            x: spin("X offset", -20000.0, 20000.0, 1.0, 0),
            y: spin("Y offset", -20000.0, 20000.0, 1.0, 0),
            scale: slider(MIN_SCALE.ln(), 2f64.ln(), 0.01),
            exact: expander("Exact size"),
            width: spin("Width", 1.0, 20000.0, 1.0, 0),
            height: spin("Height", 1.0, 20000.0, 1.0, 0),
            opacity: slider(0.0, 100.0, 1.0),
            speed: spin("Speed", 0.1, 4.0, 0.1, 1),
            layer: combo(
                "Layer",
                &[&tr("Above everything"), &tr("Above windows"), &tr("Below windows"), &tr("Desktop background")],
            ),
            looping: switch("Loop"),
            volume: slider(0.0, 100.0, 1.0),
            visible: switch("Visible"),
            playing: switch("Playing"),
            fps: expander("Limit frame rate"),
            fps_value: spin("Frames per second", 1.0, 240.0, 1.0, 0),
            decode: combo("Decoding", &[&tr("Automatic"), &tr("Hardware"), &tr("Software")]),
            optimize: switch("Optimized copy"),
            overflow: switch("Overflow"),
        });
        this.build();
        this.wire();
        this
    }

    fn build(&self) {
        let names = [
            tr("Top left"),
            tr("Top"),
            tr("Top right"),
            tr("Left"),
            tr("Center"),
            tr("Right"),
            tr("Bottom left"),
            tr("Bottom"),
            tr("Bottom right"),
        ];
        for (i, (b, name)) in self.anchors.iter().zip(&names).enumerate() {
            b.set_group(self.anchors.first().filter(|_| i > 0));
            b.set_tooltip_text(Some(name));
            b.update_property(&[gtk::accessible::Property::Label(name)]);
        }
        self.banner.set_button_label(Some(&tr("Choose another file…")));
        let ctl = self.ctl.clone();
        self.banner.connect_button_clicked(move |banner| {
            let dialog = gtk::FileDialog::builder().title(tr("Choose another file")).build();
            let ctl = ctl.clone();
            dialog.open(banner.root().and_downcast_ref::<gtk::Window>(), gtk::gio::Cancellable::NONE, move |file| {
                let path = file.ok().and_then(|f| f.path()).map(|p| p.to_string_lossy().into_owned());
                let id = ctl.selected.borrow().clone();
                if let Some((id, path)) = id.zip(path) {
                    let patch = OverlayPatch { path: Some(path), ..Default::default() };
                    ctl.send(Request::Set { id, patch, transient: false });
                }
            });
        });
        let grid = gtk::Grid::builder()
            .row_spacing(4)
            .column_spacing(4)
            .valign(gtk::Align::Center)
            .margin_top(8)
            .margin_bottom(8)
            .build();
        self.anchors.iter().enumerate().for_each(|(i, b)| grid.attach(b, i as i32 % 3, i as i32 / 3, 1, 1));
        let anchor = adw::ActionRow::builder().title(tr("Anchor")).build();
        anchor.add_suffix(&grid);
        self.exact.add_row(&self.width);
        self.exact.add_row(&self.height);
        self.fps.add_row(&self.fps_value);
        let scale = slider_row("Scale", &self.scale, |v| format!("{:.2}×", v.exp()));
        self.scale.set_width_request(150);
        self.scale.set_hexpand(true);
        let opacity = slider_row("Opacity", &self.opacity, |v| format!("{v:.0} %"));
        let volume = slider_row("Volume", &self.volume, |v| if v < 1.0 { tr("No audio") } else { format!("{v:.0}") });
        let w = |r: &dyn AsRef<gtk::Widget>| r.as_ref().clone();
        let page = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(20)
            .margin_top(16)
            .margin_bottom(24)
            .margin_start(16)
            .margin_end(16)
            .build();
        let preview = gtk::Overlay::builder().child(&self.preview).build();
        preview.add_overlay(&self.preview_toggle);
        let header = gtk::Box::new(gtk::Orientation::Vertical, 3);
        header.append(&preview);
        header.append(&self.title);
        header.append(&self.subtitle);
        header.append(&self.details);
        self.title.set_margin_top(10);
        page.append(&header);
        page.append(&self.banner);
        self.layer.set_subtitle(&tr("Above everything also covers fullscreen apps; Above windows stays under them."));
        self.overflow.set_subtitle(&tr(
            "Lets up to half of the overlay go past the monitor edges, to hide transparent margins. Off brings it back \
             inside.",
        ));
        let place = [w(&self.output), w(&anchor), w(&self.x), w(&self.y), w(&self.overflow)];
        page.append(&group("Placement", "aniway-placement-symbolic", &place.iter().collect::<Vec<_>>()));
        let look = [w(&scale), w(&self.exact), w(&opacity), w(&self.speed)];
        page.append(&group("Size and look", "aniway-size-symbolic", &look.iter().collect::<Vec<_>>()));
        let playback = [w(&self.layer), w(&self.looping), w(&volume), w(&self.visible), w(&self.playing)];
        page.append(&group("Playback", "media-playback-start-symbolic", &playback.iter().collect::<Vec<_>>()));
        let perf = [w(&self.fps), w(&self.decode), w(&self.optimize)];
        page.append(&group("Performance", "power-profile-performance-symbolic", &perf.iter().collect::<Vec<_>>()));
        self.widget.add_css_class("inspector");
        self.widget.set_width_request(330);
        self.widget.add_named(
            &gtk::ScrolledWindow::builder().child(&page).hscrollbar_policy(gtk::PolicyType::Never).build(),
            Some("edit"),
        );
        self.widget.add_named(&self.none, Some("none"));
    }

    /// Shows `path` in the preview: its thumbnail at once, then, for moving media, a loop of small frames cut by
    /// ffmpeg (cached), flipped by a timer. Decoding the file itself here would cost as much as the overlay.
    fn show_preview(self: &Rc<Self>, media: Option<(&str, &Media)>) {
        let path = media.map_or("", |m| m.0);
        if *self.preview_path.borrow() == path {
            return;
        }
        *self.preview_path.borrow_mut() = path.to_string();
        *self.preview_frames.borrow_mut() = Rc::default();
        self.preview.set_paintable(media.and_then(|m| m.1.texture.as_ref()));
        let moving = media.is_some_and(|(p, m)| m.moving() && !paths::is_url(p));
        self.preview_toggle.set_visible(false);
        self.preview_on.set(self.preview.settings().is_gtk_enable_animations());
        self.sync_preview();
        if moving {
            let (this, path) = (Rc::downgrade(self), path.to_string());
            glib::spawn_future_local(async move {
                let frames = media::preview_frames(path.clone()).await;
                let Some(this) = this.upgrade().filter(|t| *t.preview_path.borrow() == path) else { return };
                this.preview_toggle.set_visible(frames.len() > 1);
                *this.preview_frames.borrow_mut() = Rc::new(frames);
                this.sync_preview();
            });
        }
    }

    /// Plays the preview only while wanted, on screen and loaded.
    fn sync_preview(&self) {
        let on = self.preview_on.get();
        let frames = self.preview_frames.borrow().clone();
        let play = on && self.preview.is_mapped() && frames.len() > 1;
        let running = self.preview_timer.borrow().is_some();
        match (play, running) {
            (true, false) => {
                let (picture, mut i) = (self.preview.clone(), 0);
                let id = glib::timeout_add_local(Duration::from_millis(1000 / u64::from(PREVIEW_FPS)), move || {
                    i = (i + 1) % frames.len();
                    picture.set_paintable(Some(&frames[i]));
                    glib::ControlFlow::Continue
                });
                *self.preview_timer.borrow_mut() = Some(id);
            }
            (false, true) => {
                if let Some(id) = self.preview_timer.borrow_mut().take() {
                    id.remove();
                }
            }
            _ => {}
        }
        let icon = if on { "media-playback-pause-symbolic" } else { "media-playback-start-symbolic" };
        self.preview_toggle.set_icon_name(icon);
        self.preview_toggle.set_tooltip_text(Some(&if on { tr("Pause the preview") } else { tr("Play the preview") }));
    }

    /// Logarithmic scale track from `MIN_SCALE` to `fit` (the monitor's size), marked at ¼, ½ and 1× when they fit,
    /// so small sizes get as much room as large ones.
    fn set_scale_range(&self, fit: f64) {
        let upper = fit.max(MIN_SCALE * 4.0).ln();
        if (self.scale.adjustment().upper() - upper).abs() < 1e-6 {
            return;
        }
        self.scale.set_range(MIN_SCALE.ln(), upper);
        self.scale.clear_marks();
        for mark in [0.25f64, 0.5, 1.0].into_iter().filter(|m| m.ln() < upper - 0.05) {
            self.scale.add_mark(mark.ln(), gtk::PositionType::Bottom, None);
        }
        self.scale.add_mark(upper, gtk::PositionType::Bottom, None);
    }

    /// Offset that keeps the selected overlay where it is drawn, moved back inside its monitor.
    fn inside_offset(&self) -> Option<(i32, i32)> {
        let state = self.ctl.state.borrow();
        let state = state.as_ref()?;
        let id = self.ctl.selected.borrow().clone()?;
        let ov = state.overlays.iter().find(|o| o.config.id == id)?;
        let name = ov.config.output.as_deref();
        let out = state.outputs.iter().find(|o| Some(o.name.as_str()) == name).or(state.outputs.first())?;
        let bounds = Size { w: out.rect.w, h: out.rect.h };
        let media = ov.media.or_else(|| self.ctl.media.borrow().get(&ov.config.path).and_then(|m| m.size))?;
        let inside = Overlay { overflow: false, ..ov.config.clone() };
        let r = place(bounds, media, &inside);
        Some(offset_for(bounds, Size { w: r.w, h: r.h }, Point { x: r.x, y: r.y }, inside.anchor))
    }

    fn edit(&self, f: impl FnOnce(&mut OverlayPatch)) {
        if !self.updating.get() {
            let mut patch = OverlayPatch::default();
            f(&mut patch);
            self.ctl.edit(patch);
        }
    }

    fn wire(self: &Rc<Self>) {
        self.preview_toggle.connect_clicked(glib::clone!(
            #[weak(rename_to = this)]
            self,
            move |_| {
                this.preview_on.set(!this.preview_on.get());
                this.sync_preview();
            }
        ));
        let this = Rc::downgrade(self);
        self.preview.connect_map(move |_| this.upgrade().map_or((), |t| t.sync_preview()));
        let this = Rc::downgrade(self);
        self.preview.connect_unmap(move |_| this.upgrade().map_or((), |t| t.sync_preview()));
        macro_rules! on {
            ($w:ident, $signal:ident, |$t:ident, $r:ident, $p:ident| $body:expr) => {
                self.$w.$signal(glib::clone!(
                    #[weak(rename_to = $t)]
                    self,
                    move |$r| $t.edit(|$p| $body)
                ))
            };
        }
        on!(output, connect_selected_notify, |t, r, p| p.output =
            t.outputs.borrow().get(r.selected() as usize).cloned());
        // Turning overflow off also rewrites the offset to where the overlay is drawn inside the monitor, so the
        // numbers match what is seen and turning it on again starts from there.
        self.overflow.connect_active_notify(glib::clone!(
            #[weak(rename_to = t)]
            self,
            move |r| {
                let on = r.is_active();
                let back = if on { None } else { t.inside_offset() };
                t.edit(|p| {
                    p.overflow = Some(on);
                    if let Some((x, y)) = back {
                        (p.x, p.y) = (Some(x), Some(y));
                    }
                });
            }
        ));
        on!(x, connect_value_notify, |t, r, p| p.x = Some(r.value() as i32));
        on!(y, connect_value_notify, |t, r, p| p.y = Some(r.value() as i32));
        on!(scale, connect_value_changed, |t, r, p| p.scale = Some((r.value().exp() * 1000.0).round() / 1000.0));
        on!(opacity, connect_value_changed, |t, r, p| p.opacity = Some(r.value() / 100.0));
        on!(speed, connect_value_notify, |t, r, p| p.speed = Some((r.value() * 10.0).round() / 10.0));
        on!(layer, connect_selected_notify, |t, r, p| p.layer = LAYERS.get(r.selected() as usize).copied());
        on!(looping, connect_active_notify, |t, r, p| p.looping = Some(r.is_active()));
        on!(volume, connect_value_changed, |t, r, p| p.volume = Some(r.value() as u32));
        on!(visible, connect_active_notify, |t, r, p| p.visible = Some(r.is_active()));
        on!(playing, connect_active_notify, |t, r, p| p.playing = Some(r.is_active()));
        on!(decode, connect_selected_notify, |t, r, p| p.decode = DECODE.get(r.selected() as usize).copied());
        on!(optimize, connect_active_notify, |t, r, p| p.optimize = Some(r.is_active()));
        on!(width, connect_value_notify, |t, r, p| p.width = Some(r.value() as u32));
        on!(height, connect_value_notify, |t, r, p| p.height = Some(r.value() as u32));
        on!(fps_value, connect_value_notify, |t, r, p| p.max_fps =
            t.fps.enables_expansion().then_some(r.value() as u32));
        on!(fps, connect_enable_expansion_notify, |t, r, p| p.max_fps =
            Some(if r.enables_expansion() { t.fps_value.value() as u32 } else { 0 }));
        on!(exact, connect_enable_expansion_notify, |t, r, p| {
            let on = r.enables_expansion();
            p.width = Some(if on { t.width.value() as u32 } else { 0 });
            p.height = Some(if on { t.height.value() as u32 } else { 0 });
        });
        for (anchor, button) in ANCHORS.into_iter().zip(&self.anchors) {
            let this = self.clone();
            button.connect_toggled(move |b| {
                if b.is_active() {
                    this.edit(|p| p.anchor = Some(anchor));
                }
            });
        }
    }

    pub fn update(self: &Rc<Self>) {
        let state = self.ctl.state.borrow();
        let selected = self.ctl.selected.borrow();
        let overlay = state
            .as_ref()
            .zip(selected.as_ref())
            .and_then(|(s, id)| s.overlays.iter().find(|o| &o.config.id == id).map(|o| (s, o)));
        let Some((state, ov)) = overlay else {
            let empty = state.as_ref().is_some_and(|s| s.overlays.is_empty());
            self.none.set_title(&if empty { tr("Add media to get started") } else { tr("Select an overlay") });
            self.none.set_description(Some(&if empty {
                tr("Drop videos, GIFs or images on the list, or use Add in the header bar.")
            } else {
                tr("Pick one in the list or on the map.")
            }));
            self.none.set_icon_name(Some(if empty { crate::theme::logo() } else { "aniway-select-symbolic" }));
            self.show_preview(None);
            self.widget.set_visible_child_name("none");
            return;
        };
        self.widget.set_visible_child_name("edit");
        if self.ctl.editing() {
            // While dragging, the offsets follow the sticker without being sent back.
            let live = self.ctl.view().and_then(|v| v.overlays.into_iter().find(|o| o.config.id == ov.config.id));
            if let Some(live) = live.filter(|_| self.ctl.holding.get()) {
                self.updating.set(true);
                self.x.set_value(f64::from(live.config.x));
                self.y.set_value(f64::from(live.config.y));
                self.updating.set(false);
            }
            return;
        }
        self.updating.set(true);
        self.fill(state, ov);
        self.updating.set(false);
    }

    fn fill(self: &Rc<Self>, state: &StateData, ov: &OverlayInfo) {
        let c = &ov.config;
        self.title.set_text(file_name(&c.path));
        self.subtitle.set_text(&c.path);
        let media = self.ctl.media.borrow().get(&c.path).cloned().unwrap_or_default();
        let size = ov.media.or(media.size);
        let mut details = vec![size.map_or(tr("size unknown"), |m| format!("{}×{}", m.w, m.h))];
        details.extend(media.codec.clone());
        details.push(match (media.duration.is_some(), media.moving()) {
            (true, _) => media.duration_text(),
            (false, true) => tr("animation"),
            _ => tr("still image"),
        });
        details.extend(ov.hwdec.iter().filter(|d| *d != "no").cloned());
        details.push(
            ngettext("{} dropped frame", "{} dropped frames", ov.dropped as u32).replace("{}", &ov.dropped.to_string()),
        );
        self.details.set_text(&details.join(" · "));
        let probed = media.size.is_some() || media.texture.is_some();
        self.show_preview(probed.then_some((c.path.as_str(), &media)));
        self.banner.set_title(ov.error.as_deref().unwrap_or(""));
        self.banner.set_revealed(ov.error.is_some());

        let names: Vec<String> = state.outputs.iter().map(|o| o.name.clone()).collect();
        if *self.outputs.borrow() != names {
            self.output.set_model(Some(&gtk::StringList::new(&names.iter().map(String::as_str).collect::<Vec<_>>())));
            *self.outputs.borrow_mut() = names;
        }
        let index =
            |name: &Option<String>| self.outputs.borrow().iter().position(|n| Some(n) == name.as_ref()).unwrap_or(0);
        self.output.set_selected(index(&c.output) as u32);
        self.anchors[ANCHORS.iter().position(|a| *a == c.anchor).unwrap_or(0)].set_active(true);
        self.x.set_value(f64::from(c.x));
        self.y.set_value(f64::from(c.y));
        self.overflow.set_active(c.overflow);
        self.set_scale_range(fit_scale(state, ov, size));
        self.scale.set_value(c.scale.max(MIN_SCALE).ln());
        let size = ov.rect.unwrap_or_default();
        self.exact.set_enable_expansion(c.width.is_some() || c.height.is_some());
        self.width.set_value(f64::from(c.width.unwrap_or(size.w)));
        self.height.set_value(f64::from(c.height.unwrap_or(size.h)));
        self.opacity.set_value(c.opacity * 100.0);
        self.speed.set_value(c.speed);
        self.layer.set_selected(LAYERS.iter().position(|l| *l == c.layer).unwrap_or(0) as u32);
        self.looping.set_active(c.looping);
        self.volume.set_value(f64::from(c.volume));
        self.visible.set_active(c.visible);
        self.playing.set_active(c.playing);
        self.fps.set_enable_expansion(c.max_fps.is_some());
        self.fps_value.set_value(f64::from(c.max_fps.unwrap_or(30)));
        self.decode.set_selected(DECODE.iter().position(|d| *d == c.decode).unwrap_or(0) as u32);
        self.optimize.set_active(c.optimize);
        self.optimize.set_subtitle(&match (ov.proxy, &ov.proxy_error) {
            (ProxyState::Building, _) => tr("Building the optimized copy"),
            (ProxyState::Ready, _) => tr("Optimized copy ready"),
            (ProxyState::Failed, e) => {
                tr("Failed: {}").replace("{}", &e.clone().unwrap_or_else(|| tr("unknown error")))
            }
            (ProxyState::Off, _) => String::new(),
        });
    }
}
