use crate::egl::Egl;
use crate::mpv::{Event, Player, Source};
use crate::stats::{self, Counter};
use crate::surface::OverlaySurface;
use crate::App;
use aniway_proto::geometry::{place, Size};
use aniway_proto::model::Overlay;
use calloop::timer::{TimeoutAction, Timer};
use smithay_client_toolkit::output::OutputInfo as WlOutputInfo;
use smithay_client_toolkit::reexports::client::protocol::{wl_output::WlOutput, wl_surface::WlSurface};
use smithay_client_toolkit::shell::wlr_layer::LayerSurface;
use smithay_client_toolkit::shell::WaylandSurface;
use std::collections::HashMap;
use std::time::Duration;

/// Time after which a loaded medium without any video frame counts as undecodable.
const DECODE_TIMEOUT: Duration = Duration::from_secs(5);

/// Why a player could not be created, remembered so it is not retried on every reconcile.
pub(crate) struct Failure {
    pub path: String,
    pub hwdec: String,
    pub error: String,
}

/// An overlay in the scene together with its player and, while visible, its surface.
pub(crate) struct Live {
    pub key: u64,
    pub player: Player,
    pub applied: Overlay,
    pub surface: Option<OverlaySurface>,
    pub media: Option<Size>,
    /// Frame rate of the original media, once known.
    pub fps: Option<f64>,
    /// Duration of the original media in seconds, once known.
    pub duration: Option<f64>,
    /// The media has transparency; known once the player reports its first frame size.
    pub alpha: Option<bool>,
    /// Optimized copy being played instead of the original.
    pub source: Option<String>,
    /// The `fps` filter is active.
    pub capped: bool,
    pub error: Option<String>,
    pub paused: bool,
    /// The player has produced a video frame; without one the media is reported as undecodable.
    pub decoded: bool,
}

impl Live {
    pub(crate) fn destroy(mut self, egl: &Egl) {
        if let Some(s) = self.surface.take() {
            s.destroy(egl);
        }
        let _ = egl.make_current(None);
    }
}

impl App {
    /// Brings players and surfaces in line with the desired scene and runtime state.
    pub(crate) fn reconcile(&mut self) {
        self.notify_state();
        self.fill_default_output();
        let gone: Vec<String> = self.live.keys().filter(|id| self.scene.find(id).is_none()).cloned().collect();
        for id in gone {
            if let Some(live) = self.live.remove(&id) {
                live.destroy(&self.egl);
            }
        }
        self.failed.retain(|id, _| self.scene.find(id).is_some());
        for ov in self.scene.overlays.clone() {
            self.sync(&ov);
        }
        self.proxy_review();
    }

    /// Monitor with focus according to Hyprland, else the first output by name.
    pub(crate) fn default_output(&self) -> Option<String> {
        let focused = self.hypr.as_ref().and_then(|h| h.focused_monitor());
        focused.or_else(|| self.wl.find_output(None).and_then(|(_, i)| i.name))
    }

    /// Overlays without an output get the default one in memory; it is persisted with the next save.
    pub(crate) fn fill_default_output(&mut self) {
        if self.scene.overlays.iter().any(|o| o.output.is_none()) {
            if let Some(name) = self.default_output() {
                self.scene
                    .overlays
                    .iter_mut()
                    .filter(|o| o.output.is_none())
                    .for_each(|o| o.output = Some(name.clone()));
            }
        }
    }

    /// Players exist only while an overlay can be shown: persistent hiding (`visible = false` or a missing
    /// output) destroys them, transient hiding (snooze, class rules) only pauses them.
    pub(crate) fn sync(&mut self, ov: &Overlay) {
        let changed =
            |l: &Live| (&l.applied.path, l.applied.decode, l.applied.max_fps) != (&ov.path, ov.decode, ov.max_fps);
        if self.live.get(&ov.id).is_some_and(changed) {
            self.failed.remove(&ov.id);
            self.live.remove(&ov.id).into_iter().for_each(|l| l.destroy(&self.egl));
        }
        let output = self.wl.find_output(ov.output.as_deref());
        let Some(output) = output.filter(|_| ov.visible) else {
            self.live.remove(&ov.id).into_iter().for_each(|l| l.destroy(&self.egl));
            return;
        };
        let Some(mut live) = self.live.remove(&ov.id).or_else(|| self.spawn(ov)) else {
            return;
        };
        self.update(&mut live, ov, output);
        self.live.insert(ov.id.clone(), live);
    }

    pub(crate) fn spawn(&mut self, ov: &Overlay) -> Option<Live> {
        if self.failed.get(&ov.id).is_some_and(|f| f.path == ov.path && f.hwdec == self.config.hwdec) {
            return None;
        }
        let _ = self.egl.make_current(None);
        self.next_key += 1;
        // A built copy is played from the start; only the size and rate of the original are then taken from ffprobe.
        let ready = self.proxy_source(ov);
        let (source, media, fps) = match ready {
            Some(file) => (Some(file), self.sizes.get(&ov.path).copied(), None),
            None => match self.proxy_cached(ov) {
                Some((file, size, fps)) => (Some(file), Some(size), Some(fps)),
                None => (None, None, None),
            },
        };
        if let Some(size) = media {
            self.sizes.insert(ov.path.clone(), size);
        }
        let big = source.is_none() && self.sizes.get(&ov.path).is_some_and(|s| s.h >= self.config.hwdec_min_height);
        let load = Source { path: source.clone().unwrap_or_else(|| ov.path.clone()), known_big: big };
        let display = self.wl.display_ptr();
        match Player::new(ov, &load, &self.config.hwdec, true, display, self.mpv_tx.clone(), self.next_key) {
            Ok(player) => {
                self.failed.remove(&ov.id);
                if source.is_some() {
                    log::info!("overlay '{}' plays its optimized copy", ov.id);
                }
                Some(Live {
                    key: self.next_key,
                    player,
                    applied: ov.clone(),
                    surface: None,
                    media,
                    fps,
                    duration: None,
                    alpha: None,
                    source,
                    capped: false,
                    error: None,
                    paused: true,
                    decoded: false,
                })
            }
            Err(e) => {
                log::error!("overlay '{}': {e:#}", ov.id);
                let failure =
                    Failure { path: ov.path.clone(), hwdec: self.config.hwdec.clone(), error: format!("{e:#}") };
                self.failed.insert(ov.id.clone(), failure);
                None
            }
        }
    }

    pub(crate) fn update(&mut self, live: &mut Live, ov: &Overlay, output: (WlOutput, WlOutputInfo)) {
        let old = &live.applied;
        if old.speed != ov.speed {
            live.player.set_speed(ov.speed);
        }
        if old.looping != ov.looping {
            live.player.set_loop(ov.looping);
        }
        if old.volume != ov.volume {
            live.player.set_volume(ov.volume);
        }
        live.applied = ov.clone();

        let name = output.1.name.clone().unwrap_or_default();
        // Fullscreen windows need no rule: the layer decides whether an overlay is drawn above them.
        let hidden = self.hidden || self.snooze.is_some() || self.screen.class_hidden.contains(&name);
        let paused = !ov.playing || self.paused_all || hidden;
        if paused != live.paused {
            live.player.set_paused(paused);
            live.paused = paused;
        }

        let target = (!hidden).then_some(output).zip(live.media);
        if let Some(s) = &live.surface {
            if target.is_none() || s.output != name || s.layer_kind != ov.layer {
                if let Some(s) = live.surface.take() {
                    s.destroy(&self.egl);
                }
            }
        }
        let Some(((wl_output, info), media)) = target else {
            return;
        };
        let bounds = output_size(&info);
        let rect = place(bounds, media, ov);
        match &mut live.surface {
            Some(s) => {
                if s.differs(rect, bounds, ov.opacity) {
                    s.apply(rect, bounds, ov.opacity);
                    s.request_render(&self.egl, &live.player, &self.wl.qh);
                }
            }
            None => match OverlaySurface::new(
                &self.wl,
                &self.egl,
                &wl_output,
                &name,
                ov,
                (rect, bounds),
                info.scale_factor.max(1) as u32 * 120,
            ) {
                Ok(s) => live.surface = Some(s),
                Err(e) => log::error!("overlay '{}': {e:#}", ov.id),
            },
        }
    }

    pub(crate) fn on_mpv(&mut self, key: u64) {
        let Some(id) = self.live.iter().find(|(_, l)| l.key == key).map(|(id, _)| id.clone()) else {
            return;
        };
        let Some(live) = self.live.get_mut(&id) else {
            return;
        };
        let (mut resized, mut respawn) = (false, false);
        stats::count(Counter::Wakeups);
        let mut changed = false;
        for event in live.player.events() {
            match event {
                Event::Size(size) => {
                    live.decoded = true;
                    // A copy keeps the transparency and the duration of its original; the first report lets the
                    // copies be decided (ffprobe cannot tell the duration of APNG).
                    changed |= live.alpha.is_none();
                    live.alpha = Some(live.player.alpha());
                    live.duration = live.duration.or_else(|| live.player.duration());
                    // A copy has another size and frame rate: layout and decisions keep using the original's.
                    if live.source.is_some() {
                        continue;
                    }
                    resized |= live.media != Some(size);
                    live.media = Some(size);
                    live.fps = live.player.fps();
                    self.sizes.insert(live.applied.path.clone(), size);
                    respawn |=
                        live.player.hwdec_eligible && !live.player.hwdec_on && size.h >= self.config.hwdec_min_height;
                    if let Some(max) = live
                        .applied
                        .max_fps
                        .filter(|&n| !live.capped && live.fps.is_some_and(|f| f > f64::from(n) + 0.5))
                    {
                        live.player.cap_fps(max);
                        live.capped = true;
                    }
                }
                Event::Loaded => {
                    changed |= live.error.take().is_some();
                    let _ = self.handle.insert_source(Timer::from_duration(DECODE_TIMEOUT), move |_, _, app| {
                        app.check_decoded(key);
                        TimeoutAction::Drop
                    });
                }
                Event::Failed(e) => {
                    log::warn!("overlay '{id}': {e}");
                    live.error = Some(e);
                    changed = true;
                }
            }
        }
        if respawn {
            self.respawn(&id);
            return;
        }
        if live.player.frame_ready() {
            live.decoded = true;
            stats::count(Counter::NewFrames);
            if let Some(s) = &mut live.surface {
                s.request_render(&self.egl, &live.player, &self.wl.qh);
            }
        }
        if resized {
            if let Some(ov) = self.scene.find(&id).cloned() {
                self.sync(&ov);
            }
        }
        if changed || resized {
            self.proxy_review();
            self.notify_state();
        }
    }

    /// Flags media that loaded but never produced a frame (corrupt data, no video track): mpv would keep
    /// retrying forever without an error event.
    fn check_decoded(&mut self, key: u64) {
        if let Some(live) = self.live.values_mut().find(|l| l.key == key).filter(|l| !l.decoded && l.error.is_none()) {
            log::warn!("overlay '{}': no frame decoded after {DECODE_TIMEOUT:?}", live.applied.id);
            live.error = Some("could not decode the media".into());
            self.notify_state();
        }
    }

    /// Replaces the player of an overlay whose media turned out to be big, now that its size is cached so the new
    /// player starts with hardware decoding. The surface is kept so the last frame stays on screen meanwhile.
    pub(crate) fn respawn(&mut self, id: &str) {
        let Some(mut old) = self.live.remove(id) else { return };
        let (surface, media, fps, duration, alpha) = (old.surface.take(), old.media, old.fps, old.duration, old.alpha);
        old.destroy(&self.egl);
        let spawned = self.scene.find(id).cloned().and_then(|ov| self.spawn(&ov));
        match spawned {
            Some(mut new) => {
                (new.surface, new.media, new.fps, new.duration, new.alpha) = (surface, media, fps, duration, alpha);
                self.live.insert(id.to_string(), new);
            }
            None => surface.into_iter().for_each(|s| s.destroy(&self.egl)),
        }
        self.reconcile();
    }

    pub(crate) fn frame_done(&mut self, surface: &WlSurface) {
        let (egl, qh) = (&self.egl, &self.wl.qh);
        if let Some(Live { surface: Some(s), player, .. }) = by_surface(&mut self.live, surface) {
            s.frame_done(egl, player, qh);
        }
    }

    pub(crate) fn layer_configured(&mut self, layer: &LayerSurface) {
        let (egl, qh) = (&self.egl, &self.wl.qh);
        if let Some(Live { surface: Some(s), player, .. }) = by_surface(&mut self.live, layer.wl_surface()) {
            if !s.configured {
                s.configured = true;
                s.request_render(egl, player, qh);
            }
        }
    }

    pub(crate) fn layer_closed(&mut self, layer: &LayerSurface) {
        if let Some(live) = by_surface(&mut self.live, layer.wl_surface()) {
            live.surface.take().into_iter().for_each(|s| s.destroy(&self.egl));
        }
    }

    /// Applies a new buffer scale (in 1/120 units) to the surface that reported it.
    pub(crate) fn surface_scale(&mut self, surface: &WlSurface, scale120: u32, fractional: bool) {
        let (egl, qh) = (&self.egl, &self.wl.qh);
        let Some(Live { surface: Some(s), player, applied, .. }) = by_surface(&mut self.live, surface) else {
            return;
        };
        if (!fractional && s.has_fractional_scale()) || s.scale120() == scale120 {
            return;
        }
        s.set_scale120(scale120);
        let (rect, bounds) = s.rect();
        s.apply(rect, bounds, applied.opacity);
        s.request_render(egl, player, qh);
        self.proxy_review();
    }
}

pub(crate) fn by_surface<'a>(live: &'a mut HashMap<String, Live>, surface: &WlSurface) -> Option<&'a mut Live> {
    live.values_mut().find(|l| l.surface.as_ref().is_some_and(|s| s.wl_surface() == surface))
}

pub(crate) fn output_size(info: &WlOutputInfo) -> Size {
    let (w, h) =
        info.logical_size.unwrap_or_else(|| info.modes.iter().find(|m| m.current).map_or((1, 1), |m| m.dimensions));
    Size { w: w.max(1) as u32, h: h.max(1) as u32 }
}
