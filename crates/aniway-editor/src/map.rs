//! MonitorMap: every output in its real layout, the focused one as a plate at a scale shared by all outputs,
//! and the overlays of that output as die-cut stickers at their real rectangles.

use crate::client::Ctl;
use crate::theme::{self, Palette, DISPLAY, MONO, VINYL};
use crate::tr;
use adw::prelude::*;
use adw::subclass::prelude::*;
use aniway_proto::geometry::{anchor_point, offset_for, overflow_slack, place, Point, Size};
use aniway_proto::ipc::{OutputInfo, OverlayInfo, StateData};
use aniway_proto::model::{Anchor, OverlayPatch};
use gtk::{gdk, glib, graphene, gsk, pango};
use std::cell::{Cell, OnceCell, RefCell};
use std::rc::Rc;

const PAD: f32 = 20.0;
const MINI_H: f32 = 54.0;
/// Room above the overview for its heading ("Active monitors", "Scale").
const HEAD: f32 = 24.0;
/// Smallest overview rectangle: wide enough to click and to read the output name.
const MINI_MIN: (f32, f32) = (84.0, 40.0);
const SNAP_PX: f32 = 8.0;
const LID_SIDE: f32 = 16.0;
const LID_TOP: f32 = 28.0;
const LID_BOTTOM: f32 = 14.0;
const FALLBACK_MEDIA: Size = Size { w: 160, h: 90 };
/// Spacing of the dot grid on the canvas.
const DOT_STEP: f32 = 22.0;

/// An overlay being dragged: where it started (logical px on its output) and what is needed to invert `place`.
pub struct Drag {
    id: String,
    start: Point,
    origin: (i32, i32),
    size: Size,
    anchor: Anchor,
    output: Size,
    scale: f32,
    moved: bool,
    /// Overview output under the pointer, if it is not the focused one.
    over: Option<String>,
    /// Snap lines (logical px on the output) that are currently holding the overlay.
    guides: (Option<f32>, Option<f32>),
}

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct MonitorMap {
        pub ctl: OnceCell<Rc<Ctl>>,
        pub drag: RefCell<Option<Drag>>,
        pub peel: Cell<f64>,
        pub peeled: RefCell<String>,
        pub anim: RefCell<Option<adw::TimedAnimation>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for MonitorMap {
        const NAME: &'static str = "OwMonitorMap";
        type Type = super::MonitorMap;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for MonitorMap {}

    impl WidgetImpl for MonitorMap {
        fn measure(&self, orientation: gtk::Orientation, _for_size: i32) -> (i32, i32, i32, i32) {
            let min = if orientation == gtk::Orientation::Horizontal { 280 } else { 220 };
            (min, min * 3, -1, -1)
        }

        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            self.obj().draw(snapshot);
        }
    }
}

glib::wrapper! {
    pub struct MonitorMap(ObjectSubclass<imp::MonitorMap>) @extends gtk::Widget, @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

fn rect(x: f32, y: f32, w: f32, h: f32) -> graphene::Rect {
    graphene::Rect::new(x, y, w, h)
}

fn rounded(r: &graphene::Rect, radius: f32) -> gsk::RoundedRect {
    gsk::RoundedRect::from_rect(*r, radius)
}

fn out_size(o: &OutputInfo) -> Size {
    Size { w: o.rect.w, h: o.rect.h }
}

/// Output an overlay lives on; overlays without one belong to the first output.
fn owner<'a>(state: &'a StateData, ov: &OverlayInfo) -> Option<&'a OutputInfo> {
    match &ov.config.output {
        Some(name) => state.outputs.iter().find(|o| &o.name == name),
        None => state.outputs.first(),
    }
}

/// Positions in widget coordinates, shared by drawing and hit testing.
struct Geo {
    /// Output name, rectangle and whether it is a ghost (gone, but still owning overlays).
    overview: Vec<(String, graphene::Rect, bool)>,
    lid: graphene::Rect,
    screen: graphene::Rect,
    scale: f32,
}

fn geo(state: &StateData, focus: &OutputInfo, width: f32, height: f32) -> Geo {
    let min = |f: fn(&OutputInfo) -> i32| state.outputs.iter().map(f).min().unwrap_or(0) as f32;
    let max = |f: fn(&OutputInfo) -> i32| state.outputs.iter().map(f).max().unwrap_or(1) as f32;
    let (x0, y0) = (min(|o| o.rect.x), min(|o| o.rect.y));
    let (bw, bh) = (max(|o| o.rect.x + o.rect.w as i32) - x0, max(|o| o.rect.y + o.rect.h as i32) - y0);
    let fit = ((width - 2.0 * PAD - 200.0) / bw).min(MINI_H / bh);
    let readable = (MINI_MIN.0 / min(|o| o.rect.w as i32)).max(MINI_MIN.1 / min(|o| o.rect.h as i32));
    let so = fit.max(readable).min((width - 2.0 * PAD) / bw).min(3.0 * MINI_H / bh).max(0.001);
    let mut overview: Vec<_> = state
        .outputs
        .iter()
        .map(|o| {
            let (w, h) = (o.rect.w as f32 * so, o.rect.h as f32 * so);
            let (x, y) = (PAD + (o.rect.x as f32 - x0) * so, PAD + HEAD + (o.rect.y as f32 - y0) * so);
            (o.name.clone(), rect(x, y, w, h), false)
        })
        .collect();
    let mut ghosts: Vec<&str> = state.overlays.iter().filter_map(|o| o.config.output.as_deref()).collect();
    ghosts.retain(|n| !state.outputs.iter().any(|o| o.name == *n));
    ghosts.sort_unstable();
    ghosts.dedup();
    for (i, name) in ghosts.into_iter().enumerate() {
        let x = PAD + bw * so + 10.0 + i as f32 * (MINI_MIN.0 + 8.0);
        overview.push((name.to_string(), rect(x, PAD + HEAD, MINI_MIN.0, MINI_MIN.1), true));
    }
    let row_h = overview.iter().map(|(_, r, _)| r.y() + r.height()).fold(PAD + HEAD + bh * so, f32::max);
    let top = row_h + 18.0;
    let avail_w = width - 2.0 * (PAD + LID_SIDE);
    let avail_h = height - top - PAD - LID_TOP - LID_BOTTOM;
    let scale = (avail_w / max(|o| o.rect.w as i32)).min(avail_h / max(|o| o.rect.h as i32)).max(0.001);
    let (sw, sh) = (focus.rect.w as f32 * scale, focus.rect.h as f32 * scale);
    // Centred in the room left under the overview, so tall panes do not leave a gap below the plate.
    let top = top + ((avail_h - sh) / 2.0).max(0.0);
    let screen = rect((width - sw) / 2.0, top + LID_TOP, sw, sh);
    let lid = rect(screen.x() - LID_SIDE, top, sw + 2.0 * LID_SIDE, sh + LID_TOP + LID_BOTTOM);
    Geo { overview, lid, screen, scale }
}

/// Rectangle of an overlay in widget coordinates.
fn overlay_rect(g: &Geo, out: &OutputInfo, ov: &OverlayInfo) -> graphene::Rect {
    let r = place(out_size(out), ov.media.unwrap_or(FALLBACK_MEDIA), &ov.config);
    rect(
        g.screen.x() + r.x as f32 * g.scale,
        g.screen.y() + r.y as f32 * g.scale,
        r.w as f32 * g.scale,
        r.h as f32 * g.scale,
    )
}

/// Name of the real (not ghost) output of the overview under `point`.
fn output_at<'a>(g: &'a Geo, point: &graphene::Point) -> Option<&'a str> {
    g.overview.iter().find(|(_, r, ghost)| !ghost && r.contains_point(point)).map(|(name, _, _)| name.as_str())
}

/// Sticker colour class of the overlay at `index`, shared by the list and the map.
pub fn vinyl_index(index: usize) -> usize {
    index % VINYL.len()
}

/// Overlays living on `focus`, in scene order.
fn on_output<'a>(state: &'a StateData, focus: &'a OutputInfo) -> impl Iterator<Item = (usize, &'a OverlayInfo)> {
    state.overlays.iter().enumerate().filter(|(_, ov)| owner(state, ov).is_some_and(|o| o.name == focus.name))
}

/// Topmost overlay of the focused output under `point`.
fn top_overlay<'a>(
    state: &'a StateData,
    focus: &'a OutputInfo,
    g: &Geo,
    point: &graphene::Point,
) -> Option<&'a OverlayInfo> {
    on_output(state, focus).map(|(_, ov)| ov).filter(|ov| overlay_rect(g, focus, ov).contains_point(point)).last()
}

/// Snaps the span `[pos, pos + len]` inside `total`: its start, middle or end against the start, middle or end of
/// the output. Returns the snapped position and the line that holds it.
fn snap(pos: f32, len: f32, total: f32, threshold: f32) -> Option<(f32, f32)> {
    (0..3)
        .flat_map(|k| (0..3).map(move |t| (pos + len * k as f32 / 2.0, total * t as f32 / 2.0)))
        .map(|(reference, line)| (line - reference, line))
        .filter(|(delta, _)| delta.abs() < threshold)
        .min_by(|a, b| a.0.abs().total_cmp(&b.0.abs()))
        .map(|(delta, line)| (pos + delta, line))
}

pub fn display_number(index: usize) -> String {
    format!("{:02}", index + 1)
}

pub fn file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

impl MonitorMap {
    pub fn new(ctl: Rc<Ctl>) -> Self {
        let map: Self = glib::Object::new();
        map.set_focusable(true);
        let click = gtk::GestureClick::new();
        let target = map.clone();
        click.connect_pressed(move |_, _, x, y| target.pressed(x as f32, y as f32));
        map.add_controller(click);
        let drag = gtk::GestureDrag::new();
        let target = map.clone();
        drag.connect_drag_begin(move |_, x, y| target.drag_begin(x as f32, y as f32));
        let target = map.clone();
        drag.connect_drag_update(move |g, dx, dy| target.drag_update(g, dx as f32, dy as f32));
        let target = map.clone();
        drag.connect_drag_end(move |_, _, _| target.drag_end());
        map.add_controller(drag);
        let keys = gtk::EventControllerKey::new();
        let target = map.clone();
        keys.connect_key_pressed(move |_, key, _, mods| target.key(key, mods));
        map.add_controller(keys);
        let _ = map.imp().ctl.set(ctl);
        map
    }

    fn ctl(&self) -> &Rc<Ctl> {
        self.imp().ctl.get().expect("ctl is set in new")
    }

    /// The state with local edits applied, plus the focused output.
    fn current(&self) -> Option<(StateData, OutputInfo)> {
        let state = self.ctl().view()?;
        let name = self.ctl().focus.borrow().clone()?;
        let focus = state.outputs.iter().find(|o| o.name == name)?.clone();
        Some((state, focus))
    }

    fn geo(&self, state: &StateData, focus: &OutputInfo) -> Geo {
        geo(state, focus, self.width() as f32, self.height() as f32)
    }

    /// Redraws and refreshes the accessible description of the selection.
    pub fn refresh(&self) {
        self.queue_draw();
        let label = self.current().and_then(|(state, _)| {
            let id = self.ctl().selected.borrow().clone()?;
            let ov = state.overlays.iter().find(|o| o.config.id == id)?;
            let c = &ov.config;
            let out = owner(&state, ov).map_or("", |o| o.name.as_str());
            Some(
                tr("{} on {}, x {}, y {}")
                    .replacen("{}", file_name(&c.path), 1)
                    .replacen("{}", out, 1)
                    .replacen("{}", &c.x.to_string(), 1)
                    .replacen("{}", &c.y.to_string(), 1),
            )
        });
        self.update_property(&[gtk::accessible::Property::Label(label.as_deref().unwrap_or(&tr("Monitor map")))]);
    }

    fn pressed(&self, x: f32, y: f32) {
        self.grab_focus();
        let Some((state, focus)) = self.current() else { return };
        let g = self.geo(&state, &focus);
        let point = graphene::Point::new(x, y);
        let hit_output = output_at(&g, &point);
        let hit_overlay = top_overlay(&state, &focus, &g, &point);
        match (hit_output, hit_overlay) {
            (Some(name), _) => self.ctl().focus_output(name),
            (None, ov) if ov.is_some() || g.lid.contains_point(&point) => {
                self.ctl().select(ov.map(|o| o.config.id.clone()))
            }
            _ => {}
        }
    }

    fn drag_begin(&self, x: f32, y: f32) {
        let Some((state, focus)) = self.current() else { return };
        let g = self.geo(&state, &focus);
        let Some(ov) = top_overlay(&state, &focus, &g, &graphene::Point::new(x, y)) else { return };
        let r = place(out_size(&focus), ov.media.unwrap_or(FALLBACK_MEDIA), &ov.config);
        *self.imp().drag.borrow_mut() = Some(Drag {
            id: ov.config.id.clone(),
            start: Point { x: r.x, y: r.y },
            origin: (ov.config.x, ov.config.y),
            size: Size { w: r.w, h: r.h },
            anchor: ov.config.anchor,
            output: out_size(&focus),
            scale: g.scale,
            moved: false,
            over: None,
            guides: (None, None),
        });
        *self.imp().peeled.borrow_mut() = ov.config.id.clone();
        self.ctl().holding.set(true);
        self.ctl().select(Some(ov.config.id.clone()));
        self.peel_to(1.0, 150);
    }

    /// Animates the peel amount of the grabbed sticker (instant under reduced motion).
    fn peel_to(&self, to: f64, ms: u32) {
        let weak = self.downgrade();
        let target = adw::CallbackAnimationTarget::new(move |v| {
            if let Some(map) = weak.upgrade() {
                map.imp().peel.set(v);
                map.queue_draw();
            }
        });
        let anim = adw::TimedAnimation::builder()
            .widget(self)
            .value_from(self.imp().peel.get())
            .value_to(to)
            .duration(ms)
            .easing(adw::Easing::EaseOutCubic)
            .target(&target)
            .build();
        anim.play();
        *self.imp().anim.borrow_mut() = Some(anim);
    }

    /// Moves the dragged overlay by editing its offset; the controller throttles and commits the writes.
    fn drag_update(&self, gesture: &gtk::GestureDrag, dx: f32, dy: f32) {
        let Some((state, focus)) = self.current() else { return };
        let pointer = gesture.start_point().map(|(x, y)| graphene::Point::new(x as f32 + dx, y as f32 + dy));
        let over = pointer
            .and_then(|p| output_at(&self.geo(&state, &focus), &p).map(String::from))
            .filter(|n| *n != focus.name);
        let mut drag = self.imp().drag.borrow_mut();
        let Some(d) = drag.as_mut() else { return };
        d.moved |= dx.abs() + dy.abs() >= 3.0;
        d.over = over;
        if !d.moved {
            return;
        }
        let free = gesture.current_event_state().contains(gdk::ModifierType::ALT_MASK);
        let at = (d.start.x as f32 + dx / d.scale, d.start.y as f32 + dy / d.scale);
        let threshold = SNAP_PX / d.scale;
        let sx = (!free).then(|| snap(at.0, d.size.w as f32, d.output.w as f32, threshold)).flatten();
        let sy = (!free).then(|| snap(at.1, d.size.h as f32, d.output.h as f32, threshold)).flatten();
        d.guides = (sx.map(|s| s.1), sy.map(|s| s.1));
        // The stored offset stays within what the daemon can show, so turning overflow on or off never jumps.
        let overflow = state.overlays.iter().any(|o| o.config.id == d.id && o.config.overflow);
        let (slack_x, slack_y) = if overflow { overflow_slack(d.size) } else { (0, 0) };
        let max_x = d.output.w.saturating_sub(d.size.w) as i32 + slack_x;
        let max_y = d.output.h.saturating_sub(d.size.h) as i32 + slack_y;
        let top_left = Point {
            x: (sx.map_or(at.0, |s| s.0).round() as i32).clamp(-slack_x, max_x),
            y: (sy.map_or(at.1, |s| s.0).round() as i32).clamp(-slack_y, max_y),
        };
        let (x, y) = offset_for(d.output, d.size, top_left, d.anchor);
        drop(drag);
        self.ctl().edit(OverlayPatch { x: Some(x), y: Some(y), ..Default::default() });
    }

    /// Releasing over another output of the overview moves the overlay there, keeping anchor and offset.
    fn drag_end(&self) {
        let Some(d) = self.imp().drag.borrow_mut().take() else { return };
        self.peel_to(0.0, 200);
        let ctl = self.ctl();
        ctl.holding.set(false);
        if let Some(name) = d.over.filter(|_| d.moved) {
            ctl.edit(OverlayPatch {
                output: Some(name.clone()),
                x: Some(d.origin.0),
                y: Some(d.origin.1),
                ..Default::default()
            });
            ctl.focus_output(&name);
        }
        ctl.commit();
    }

    fn key(&self, key: gdk::Key, mods: gdk::ModifierType) -> glib::Propagation {
        use gdk::Key as K;
        let ctl = self.ctl();
        let Some((state, focus)) = self.current() else { return glib::Propagation::Proceed };
        let selected = ctl.selected.borrow().clone();
        let current = selected.as_ref().and_then(|id| state.overlays.iter().find(|o| &o.config.id == id));
        let step = if mods.contains(gdk::ModifierType::SHIFT_MASK) { 10 } else { 1 };
        let digit = key.to_unicode().and_then(|c| c.to_digit(10)).filter(|d| *d > 0);
        match (key, current) {
            (K::Left | K::Right | K::Up | K::Down, Some(ov)) => {
                let (dx, dy) = match key {
                    K::Left => (-step, 0),
                    K::Right => (step, 0),
                    K::Up => (0, -step),
                    _ => (0, step),
                };
                ctl.edit(OverlayPatch { x: Some(ov.config.x + dx), y: Some(ov.config.y + dy), ..Default::default() });
            }
            (K::Delete, Some(ov)) => crate::list::confirm_remove(self, ctl, &ov.config.id),
            (K::Tab | K::ISO_Left_Tab, _) => {
                let ids: Vec<_> = on_output(&state, &focus).map(|(_, o)| o.config.id.clone()).collect();
                let at = selected.as_ref().and_then(|s| ids.iter().position(|i| i == s));
                let next = match (key == K::Tab, at) {
                    (true, at) => Some(at.map_or(0, |i| i + 1)),
                    (false, at) => at.unwrap_or(ids.len()).checked_sub(1),
                };
                let Some(id) = next.and_then(|i| ids.get(i)) else { return glib::Propagation::Proceed };
                ctl.select(Some(id.clone()));
            }
            (_, _) if mods.contains(gdk::ModifierType::CONTROL_MASK) && digit.is_some() => {
                let Some(o) = digit.and_then(|d| state.outputs.get(d as usize - 1)) else {
                    return glib::Propagation::Proceed;
                };
                ctl.focus_output(&o.name);
            }
            _ => return glib::Propagation::Proceed,
        }
        glib::Propagation::Stop
    }

    /// Draws `text` with its left edge (or right edge when `right`) at `x`, ellipsized to `max_w` when positive.
    fn text(
        &self,
        snapshot: &gtk::Snapshot,
        text: &str,
        font: &str,
        color: &gdk::RGBA,
        (x, y): (f32, f32),
        (right, max_w): (bool, f32),
    ) {
        let layout = self.create_pango_layout(Some(text));
        layout.set_font_description(Some(&pango::FontDescription::from_string(font)));
        if max_w > 0.0 {
            layout.set_width((max_w * pango::SCALE as f32) as i32);
            layout.set_ellipsize(pango::EllipsizeMode::End);
        }
        let x = if right { x - layout.pixel_size().0 as f32 } else { x };
        snapshot.save();
        snapshot.translate(&graphene::Point::new(x, y));
        snapshot.append_layout(&layout, color);
        snapshot.restore();
    }

    /// Draws `text` centred in `r`, ellipsized to its width.
    fn text_centred(&self, snapshot: &gtk::Snapshot, text: &str, font: &str, color: &gdk::RGBA, r: &graphene::Rect) {
        let layout = self.create_pango_layout(Some(text));
        layout.set_font_description(Some(&pango::FontDescription::from_string(font)));
        layout.set_width(((r.width() - 8.0).max(1.0) * pango::SCALE as f32) as i32);
        layout.set_ellipsize(pango::EllipsizeMode::End);
        layout.set_alignment(pango::Alignment::Center);
        let h = layout.pixel_size().1 as f32;
        snapshot.save();
        snapshot.translate(&graphene::Point::new(r.x() + 4.0, r.y() + (r.height() - h) / 2.0));
        snapshot.append_layout(&layout, color);
        snapshot.restore();
    }

    fn draw(&self, snapshot: &gtk::Snapshot) {
        let Some((state, focus)) = self.current() else { return };
        let ctl = self.ctl();
        let p = theme::palette();
        let (width, height) = (self.width() as f32, self.height() as f32);
        let g = geo(&state, &focus, width, height);
        let drag = self.imp().drag.borrow();
        self.draw_dots(snapshot, width, height, &p);
        self.draw_overview(snapshot, &focus, &g, &p, width, drag.as_ref().and_then(|d| d.over.as_deref()));
        self.draw_plate(snapshot, &focus, &g, &p);
        if on_output(&state, &focus).next().is_none() {
            self.draw_empty(snapshot, &focus, &g);
        }
        if self.has_focus() {
            snapshot.append_border(&rounded(&g.lid, 14.0), &[2.0; 4], &[p.label; 4]);
        }
        let (selected, peeled) = (ctl.selected.borrow().clone(), self.imp().peeled.borrow().clone());
        let mut stickers: Vec<_> = on_output(&state, &focus).collect();
        stickers.sort_by_key(|(_, ov)| ov.config.id == peeled);
        for (index, ov) in stickers {
            let peel = if ov.config.id == peeled { self.imp().peel.get() as f32 } else { 0.0 };
            let texture = ctl.media.borrow().get(&ov.config.path).and_then(|m| m.texture.clone());
            let r = overlay_rect(&g, &focus, ov);
            // An overflowing sticker is cut at the screen edge like on the monitor; a dashed outline keeps the
            // hidden part findable and grabbable.
            let spills = r.intersection(&g.screen) != Some(r);
            if spills {
                let path = gsk::PathBuilder::new();
                path.add_rect(&r);
                snapshot.append_stroke(&path.to_path(), &gsk::Stroke::builder(1.5).dash(&[5.0, 4.0]).build(), &p.mute);
                snapshot.push_clip(&g.screen);
            }
            self.draw_sticker(
                snapshot,
                (index, ov),
                texture.as_ref(),
                &r,
                (selected.as_ref() == Some(&ov.config.id), peel),
                &p,
            );
            if spills {
                snapshot.pop();
            }
        }
        if let Some(d) = drag.as_ref().filter(|d| d.moved) {
            if let Some(ov) = state.overlays.iter().find(|o| o.config.id == d.id) {
                self.draw_guides(snapshot, d, &g, &p);
                self.draw_dimensions(snapshot, ov, &overlay_rect(&g, &focus, ov), &g, &p);
            }
        }
    }

    fn draw_overview(
        &self,
        s: &gtk::Snapshot,
        focus: &OutputInfo,
        g: &Geo,
        p: &Palette,
        width: f32,
        over: Option<&str>,
    ) {
        for (name, r, ghost) in &g.overview {
            let shape = rounded(r, 3.0);
            if *ghost {
                let path = gsk::PathBuilder::new();
                path.add_rounded_rect(&shape);
                s.append_stroke(&path.to_path(), &gsk::Stroke::builder(1.5).dash(&[4.0, 3.0]).build(), &p.mute);
            } else {
                s.push_rounded_clip(&shape);
                s.append_color(&p.mini, r);
                s.pop();
            }
            self.text_centred(s, name, &format!("{DISPLAY} SemiBold 9"), &p.mute, r);
            if *name == focus.name || over == Some(name) {
                s.append_border(&shape, &[2.0; 4], &[p.label; 4]);
            }
        }
        let head = format!("{DISPLAY} SemiBold 9");
        self.text(s, &tr("Active monitors"), &head, &p.mute, (PAD, PAD), (false, width - 2.0 * PAD - 120.0));
        let right = g.overview.iter().map(|(_, r, _)| r.x() + r.width()).fold(0.0, f32::max);
        if width - PAD - right > 120.0 {
            self.text(s, &tr("Scale"), &head, &p.mute, (width - PAD, PAD), (true, 0.0));
            let ratio = format!("1 : {:.2}", 1.0 / g.scale);
            self.text(s, &ratio, &format!("{MONO} Bold 11"), &p.label, (width - PAD, PAD + HEAD), (true, 0.0));
        }
    }

    /// Faint dot grid behind everything: the canvas of a design tool.
    fn draw_dots(&self, s: &gtk::Snapshot, width: f32, height: f32, p: &Palette) {
        let alpha = if p.dark { 0.07 } else { 0.12 };
        let dot = if p.dark { gdk::RGBA::new(1.0, 1.0, 1.0, alpha) } else { gdk::RGBA::new(0.0, 0.0, 0.0, alpha) };
        s.push_repeat(&rect(0.0, 0.0, width, height), Some(&rect(0.0, 0.0, DOT_STEP, DOT_STEP)));
        s.push_rounded_clip(&rounded(&rect(DOT_STEP / 2.0 - 1.0, DOT_STEP / 2.0 - 1.0, 2.0, 2.0), 1.0));
        s.append_color(&dot, &rect(DOT_STEP / 2.0 - 1.0, DOT_STEP / 2.0 - 1.0, 2.0, 2.0));
        s.pop();
        s.pop();
    }

    /// An output without overlays says so on its screen, and where media comes from.
    fn draw_empty(&self, s: &gtk::Snapshot, focus: &OutputInfo, g: &Geo) {
        let (c, w) = (g.screen.center(), g.screen.width() - 32.0);
        if w < 80.0 || g.screen.height() < 60.0 {
            return;
        }
        let title = tr("No overlays on {}").replace("{}", &focus.name);
        let hint = tr("Drop media anywhere in the window");
        let centred = |text: &str, font: &str, color: &gdk::RGBA, y: f32| {
            let layout = self.create_pango_layout(Some(text));
            layout.set_font_description(Some(&pango::FontDescription::from_string(font)));
            layout.set_width((w * pango::SCALE as f32) as i32);
            layout.set_ellipsize(pango::EllipsizeMode::End);
            layout.set_alignment(pango::Alignment::Center);
            s.save();
            s.translate(&graphene::Point::new(c.x() - w / 2.0, y));
            s.append_layout(&layout, color);
            s.restore();
        };
        centred(&title, &format!("{DISPLAY} Bold 13"), &gdk::RGBA::new(1.0, 1.0, 1.0, 0.85), c.y() - 20.0);
        centred(&hint, "DM Sans 10", &gdk::RGBA::new(1.0, 1.0, 1.0, 0.55), c.y() + 2.0);
    }

    fn draw_plate(&self, s: &gtk::Snapshot, focus: &OutputInfo, g: &Geo, p: &Palette) {
        let lid = rounded(&g.lid, 14.0);
        s.append_outset_shadow(&lid, &gdk::RGBA::new(0.0, 0.0, 0.0, 0.35), 0.0, 6.0, 0.0, 14.0);
        s.push_rounded_clip(&lid);
        s.append_color(&p.lid, &g.lid);
        s.pop();
        let (x, y, right) = (g.lid.x() + LID_SIDE, g.lid.y() + 6.0, g.lid.x() + g.lid.width() - LID_SIDE);
        let spec = format!("{} × {} · ×{:.2}", focus.rect.w, focus.rect.h, focus.scale);
        let spec_w = self.create_pango_layout(Some(&spec)).pixel_size().0 as f32 + 16.0;
        let ink = if p.dark { gdk::RGBA::new(1.0, 1.0, 1.0, 0.9) } else { theme::rgb(0x23262b) };
        let name_font = format!("{DISPLAY} Bold 10");
        self.text(s, &focus.name, &name_font, &ink, (x, y), (false, 0.0));
        let name_w = self.create_pango_layout(Some(&focus.name)).pixel_size().0 as f32 + 10.0;
        let room = right - spec_w - x - name_w;
        if room > 30.0 {
            let desc_font = format!("{DISPLAY} 9");
            self.text(s, &focus.description, &desc_font, &p.mute, (x + name_w, y + 1.0), (false, room));
        }
        self.text(s, &spec, &format!("{MONO} 9"), &p.mute, (right, y + 1.0), (true, 0.0));
        let screen = rounded(&g.screen, 8.0);
        s.push_rounded_clip(&screen);
        // CSS-style 160deg gradient: the line through the centre, as long as the box projected on it.
        let (sin, cos) = 160f32.to_radians().sin_cos();
        let (c, half) = (g.screen.center(), (g.screen.width() * sin.abs() + g.screen.height() * cos.abs()) / 2.0);
        let (from, to) = (
            graphene::Point::new(c.x() - sin * half, c.y() + cos * half),
            graphene::Point::new(c.x() + sin * half, c.y() - cos * half),
        );
        let stops = [gsk::ColorStop::new(0.0, p.screen_top), gsk::ColorStop::new(1.0, p.screen)];
        s.append_linear_gradient(&g.screen, &from, &to, &stops);
        s.pop();
    }

    /// Yellow lines from the output's anchor point to the overlay's, labelled with the offset in logical px.
    fn draw_dimensions(&self, s: &gtk::Snapshot, ov: &OverlayInfo, r: &graphene::Rect, g: &Geo, p: &Palette) {
        let c = &ov.config;
        let point = |b: &graphene::Rect| {
            let q = anchor_point(Size { w: b.width() as u32, h: b.height() as u32 }, c.anchor);
            (b.x() + q.x as f32, b.y() + q.y as f32)
        };
        let (a, b) = (point(&g.screen), point(r));
        let path = gsk::PathBuilder::new();
        path.move_to(a.0, a.1);
        path.line_to(b.0, a.1);
        path.line_to(b.0, b.1);
        s.append_stroke(&path.to_path(), &gsk::Stroke::new(1.5), &p.edge);
        let dots = gsk::PathBuilder::new();
        dots.add_circle(&graphene::Point::new(a.0, a.1), 3.5);
        dots.add_circle(&graphene::Point::new(b.0, b.1), 3.5);
        s.append_fill(&dots.to_path(), gsk::FillRule::Winding, &p.edge);
        let font = &format!("{MONO} Bold 9");
        self.text(s, &format!("x {}", c.x), font, &p.edge, ((a.0 + b.0) / 2.0, a.1 + 3.0), (false, 0.0));
        self.text(s, &format!("y {}", c.y), font, &p.edge, (b.0 + 6.0, (a.1 + b.1) / 2.0), (false, 0.0));
    }

    /// Thin lines through the edge, centre or anchor line that is holding the dragged overlay.
    fn draw_guides(&self, s: &gtk::Snapshot, d: &Drag, g: &Geo, p: &Palette) {
        let path = gsk::PathBuilder::new();
        if let Some(x) = d.guides.0 {
            let x = g.screen.x() + x * g.scale;
            path.move_to(x, g.screen.y());
            path.line_to(x, g.screen.y() + g.screen.height());
        }
        if let Some(y) = d.guides.1 {
            let y = g.screen.y() + y * g.scale;
            path.move_to(g.screen.x(), y);
            path.line_to(g.screen.x() + g.screen.width(), y);
        }
        s.append_stroke(&path.to_path(), &gsk::Stroke::builder(1.0).dash(&[4.0, 4.0]).build(), &p.edge);
    }

    fn draw_sticker(
        &self,
        s: &gtk::Snapshot,
        (index, ov): (usize, &OverlayInfo),
        texture: Option<&gdk::Texture>,
        r: &graphene::Rect,
        (selected, peel): (bool, f32),
        p: &Palette,
    ) {
        let hidden = !ov.config.visible;
        let radius = (r.width().min(r.height()) / 4.0).clamp(4.0, 14.0);
        let shape = rounded(r, radius);
        let vinyl = theme::rgb(VINYL[vinyl_index(index)]);
        if hidden {
            s.push_opacity(0.4);
        }
        // Cairo draws the shadow of a rotated shape as blocks: there it stays outside the tilt and is inset.
        let cairo = self.native().and_then(|n| n.renderer()).is_some_and(|r| r.type_().name().contains("Cairo"));
        let shadow = || {
            let color = gdk::RGBA::new(0.0, 0.0, 0.0, 0.4 + 0.133 * peel);
            let spread = if cairo { -2.0 } else { 0.0 };
            s.append_outset_shadow(&shape, &color, 0.0, 3.0 + 7.0 * peel, spread, 6.0 + 12.0 * peel);
        };
        if cairo {
            shadow();
        }
        s.save();
        let centre = graphene::Point::new(r.x() + r.width() / 2.0, r.y() + r.height() / 2.0);
        s.translate(&centre);
        // Tilt and growth are motion: with animations disabled only the lifted shadow and thicker keyline remain.
        let tilt = if self.settings().is_gtk_enable_animations() { peel } else { 0.0 };
        s.rotate(-2.0 * tilt);
        s.scale(1.0 + 0.04 * tilt, 1.0 + 0.04 * tilt);
        s.translate(&graphene::Point::new(-centre.x(), -centre.y()));
        if !cairo {
            shadow();
        }
        s.push_rounded_clip(&shape);
        s.save();
        s.translate(&centre);
        s.scale(if ov.config.flip_h { -1.0 } else { 1.0 }, if ov.config.flip_v { -1.0 } else { 1.0 });
        if ov.config.rotation != 0 {
            s.rotate(f32::from(ov.config.rotation));
        }
        s.translate(&graphene::Point::new(-centre.x(), -centre.y()));
        let texture_size =
            if matches!(ov.config.rotation, 90 | 270) { (r.height(), r.width()) } else { (r.width(), r.height()) };
        let texture_rect =
            rect(centre.x() - texture_size.0 / 2.0, centre.y() - texture_size.1 / 2.0, texture_size.0, texture_size.1);
        match texture {
            Some(texture) => s.append_scaled_texture(texture, gsk::ScalingFilter::Linear, &texture_rect),
            None => s.append_color(&gdk::RGBA::new(0.5, 0.5, 0.55, 0.35), &texture_rect),
        }
        s.restore();
        s.pop();
        // The number chip clears the rounded corner and shrinks on tiny stickers; a dark halo keeps it readable
        // on any picture.
        let small = r.width() < 64.0 || r.height() < 40.0;
        let (cw, ch, inset) = if small { (24.0, 15.0, 4.0) } else { (30.0, 19.0, radius * 0.3 + 6.0) };
        let chip = rect(r.x() + inset, r.y() + inset, cw, ch);
        s.append_outset_shadow(&rounded(&chip, ch / 2.0), &gdk::RGBA::new(0.0, 0.0, 0.0, 0.45), 0.0, 1.0, 0.0, 4.0);
        s.push_rounded_clip(&rounded(&chip, ch / 2.0));
        s.append_color(&vinyl, &chip);
        s.pop();
        let number = display_number(index);
        let font = format!("{DISPLAY} Bold {}", if small { 8 } else { 10 });
        let layout = self.create_pango_layout(Some(&number));
        layout.set_font_description(Some(&pango::FontDescription::from_string(&font)));
        let (tw, th) = layout.pixel_size();
        let at = (chip.x() + (cw - tw as f32) / 2.0, chip.y() + (ch - th as f32) / 2.0);
        self.text(s, &number, &font, &theme::rgb(theme::VINYL_INK), at, (false, 0.0));
        if texture.is_none() {
            let name = file_name(&ov.config.path);
            let x = chip.x() + cw + 6.0;
            self.text(
                s,
                name,
                &format!("{DISPLAY} Medium 9"),
                &p.keyline,
                (x, at.1),
                (false, r.x() + r.width() - x - 6.0),
            );
        }
        let width = 4.0 + 2.0 * peel;
        if hidden {
            let path = gsk::PathBuilder::new();
            path.add_rounded_rect(&shape);
            let stroke = gsk::Stroke::builder(width).dash(&[6.0, 4.0]).build();
            s.append_stroke(&path.to_path(), &stroke, &p.keyline);
        } else {
            s.append_border(&shape, &[width; 4], &[p.keyline; 4]);
        }
        if selected && peel < 1.0 {
            // Selection ring just outside the keyline; it fades while the sticker is peeled.
            let ring = rounded(&rect(r.x() - 5.0, r.y() - 5.0, r.width() + 10.0, r.height() + 10.0), radius + 5.0);
            let color = gdk::RGBA::new(p.edge.red(), p.edge.green(), p.edge.blue(), 1.0 - peel);
            s.append_border(&ring, &[2.0; 4], &[color; 4]);
        }
        s.restore();
        if hidden {
            s.pop();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aniway_proto::geometry::Rect;

    fn output(name: &str, x: i32, w: u32, h: u32) -> OutputInfo {
        OutputInfo { name: name.into(), description: String::new(), rect: Rect { x, y: 0, w, h }, scale: 1.0 }
    }

    #[test]
    fn focus_plate_keeps_true_relative_size() {
        let state = StateData {
            scene: "default".into(),
            outputs: vec![output("big", 0, 1920, 1080), output("small", 1920, 960, 540)],
            overlays: vec![],
            hidden: false,
            paused: false,
            snooze_secs_left: None,
            config: Default::default(),
        };
        let (big, small) =
            (geo(&state, &state.outputs[0], 1200.0, 800.0), geo(&state, &state.outputs[1], 1200.0, 800.0));
        assert_eq!(big.scale, small.scale);
        assert!((big.screen.width() / small.screen.width() - 2.0).abs() < 1e-4);
        assert!(big.screen.width() <= 1200.0 && big.lid.y() + big.lid.height() <= 800.0);
    }

    #[test]
    fn snapping_prefers_the_nearest_line() {
        assert_eq!(snap(3.0, 100.0, 1000.0, 8.0), Some((0.0, 0.0)));
        assert_eq!(snap(447.0, 100.0, 1000.0, 8.0), Some((450.0, 500.0)));
        assert_eq!(snap(895.0, 100.0, 1000.0, 8.0), Some((900.0, 1000.0)));
        assert_eq!(snap(200.0, 100.0, 1000.0, 8.0), None);
    }
}
