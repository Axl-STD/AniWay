use crate::egl::Egl;
use crate::mpv::Player;
use crate::stats::{self, Call, Counter};
use crate::wayland::{FractionalData, Noop, Wl};
use aniway_proto::geometry::{visible, Rect, Size};
use aniway_proto::model::{Layer, Overlay};
use anyhow::{Context, Result};
use khronos_egl as egl;
use smithay_client_toolkit::compositor::{FrameCallbackData, Region};
use smithay_client_toolkit::reexports::client::protocol::{wl_output::WlOutput, wl_surface::WlSurface};
use smithay_client_toolkit::reexports::client::{Proxy, QueueHandle};
use smithay_client_toolkit::reexports::protocols::wp::alpha_modifier::v1::client::wp_alpha_modifier_surface_v1::WpAlphaModifierSurfaceV1;
use smithay_client_toolkit::reexports::protocols::wp::fractional_scale::v1::client::wp_fractional_scale_v1::WpFractionalScaleV1;
use smithay_client_toolkit::reexports::protocols::wp::viewporter::client::wp_viewport::WpViewport;
use smithay_client_toolkit::shell::wlr_layer::{Anchor, KeyboardInteractivity, Layer as WlrLayer, LayerSurface};
use smithay_client_toolkit::shell::WaylandSurface;
use std::time::{Duration, Instant};
use wayland_egl::WlEglSurface;

const FRAME_TIMEOUT: Duration = Duration::from_secs(1);

/// A layer surface with its EGL window, one per visible overlay.
pub struct OverlaySurface {
    pub layer: LayerSurface,
    pub output: String,
    pub layer_kind: Layer,
    pub configured: bool,
    /// Set while a frame callback is outstanding; a stale value is ignored so lost callbacks cannot freeze rendering.
    frame_pending: Option<Instant>,
    pub dirty: bool,
    viewport: Option<WpViewport>,
    fractional: Option<WpFractionalScaleV1>,
    alpha: Option<WpAlphaModifierSurfaceV1>,
    window: WlEglSurface,
    surface: egl::Surface,
    scale120: u32,
    /// Full rectangle of the overlay; with overflow it may extend past `bounds` (the output), and only the part
    /// inside is shown, cropped from the full-size buffer through the viewport.
    rect: Rect,
    bounds: Size,
    opacity: f64,
}

fn wlr_layer(layer: Layer) -> WlrLayer {
    match layer {
        Layer::Overlay => WlrLayer::Overlay,
        Layer::Top => WlrLayer::Top,
        Layer::Bottom => WlrLayer::Bottom,
        Layer::Background => WlrLayer::Background,
    }
}

impl OverlaySurface {
    pub fn new(
        wl: &Wl,
        egl: &Egl,
        output: &WlOutput,
        output_name: &str,
        ov: &Overlay,
        (rect, bounds): (Rect, Size),
        scale120: u32,
    ) -> Result<Self> {
        let surface = wl.compositor.create_surface(&wl.qh);
        let layer = wl.layer_shell.create_layer_surface(
            &wl.qh,
            surface.clone(),
            wlr_layer(ov.layer),
            Some("aniway"),
            Some(output),
        );
        layer.set_anchor(Anchor::TOP | Anchor::LEFT);
        layer.set_exclusive_zone(-1);
        layer.set_keyboard_interactivity(KeyboardInteractivity::None);
        let empty = Region::new(&wl.compositor).context("wl_region")?;
        surface.set_input_region(Some(empty.wl_region()));
        let viewport = wl.viewporter.as_ref().map(|v| v.get_viewport(&surface, &wl.qh, Noop));
        let fractional =
            wl.fractional.as_ref().map(|m| m.get_fractional_scale(&surface, &wl.qh, FractionalData(surface.clone())));
        let alpha = wl.alpha.as_ref().map(|m| m.get_surface(&surface, &wl.qh, Noop));
        let window = WlEglSurface::new(surface.id(), 1, 1).context("wl_egl_window")?;
        let egl_surface = egl.create_surface(&window).context("EGL window surface")?;
        let mut this = Self {
            window,
            surface: egl_surface,
            layer,
            output: output_name.into(),
            layer_kind: ov.layer,
            configured: false,
            frame_pending: None,
            dirty: true,
            viewport,
            fractional,
            alpha,
            scale120,
            rect: Rect::default(),
            bounds,
            opacity: -1.0,
        };
        this.apply(rect, bounds, ov.opacity);
        this.layer.commit();
        Ok(this)
    }

    pub fn wl_surface(&self) -> &WlSurface {
        self.layer.wl_surface()
    }

    pub fn has_fractional_scale(&self) -> bool {
        self.fractional.is_some()
    }

    /// Buffer size in pixels: logical size times the scale, rounded up.
    fn buffer_size(&self) -> (u32, u32) {
        let px = |v: u32| (v * self.scale120).div_ceil(120).max(1);
        (px(self.rect.w), px(self.rect.h))
    }

    /// Updates position, size, scale-dependent state and opacity. The caller must render afterwards.
    pub fn apply(&mut self, rect: Rect, bounds: Size, opacity: f64) {
        // Without a viewport the buffer cannot be cropped: the surface keeps the whole rectangle and the compositor
        // decides what happens past the edge.
        let shown = if self.viewport.is_some() { visible(bounds, rect) } else { rect };
        self.layer.set_size(shown.w, shown.h);
        self.layer.set_margin(shown.y, 0, 0, shown.x);
        (self.rect, self.bounds) = (rect, bounds);
        if let Some(v) = &self.viewport {
            v.set_destination(shown.w as i32, shown.h as i32);
            if shown == rect {
                v.set_source(-1.0, -1.0, -1.0, -1.0);
            } else {
                // Buffer pixels of the visible part; the buffer is the whole overlay at the surface scale.
                let s = f64::from(self.scale120) / 120.0;
                let at = |v: i32| f64::from(v) * s;
                let (bw, bh) = self.buffer_size();
                let (x, y) = (at(shown.x - rect.x), at(shown.y - rect.y));
                let (w, h) = ((at(shown.w as i32)).min(f64::from(bw) - x), (at(shown.h as i32)).min(f64::from(bh) - y));
                v.set_source(x, y, w, h);
            }
        } else {
            self.scale120 = self.scale120.div_ceil(120) * 120;
            self.wl_surface().set_buffer_scale((self.scale120 / 120) as i32);
        }
        let (w, h) = self.buffer_size();
        self.window.resize(w as i32, h as i32, 0, 0);
        if let Some(a) = &self.alpha {
            if opacity != self.opacity {
                a.set_multiplier((opacity.clamp(0.0, 1.0) * f64::from(u32::MAX)) as u32);
            }
        }
        self.opacity = opacity;
        self.dirty = true;
    }

    pub fn differs(&self, rect: Rect, bounds: Size, opacity: f64) -> bool {
        rect != self.rect || bounds != self.bounds || opacity != self.opacity
    }

    pub fn scale120(&self) -> u32 {
        self.scale120
    }

    pub fn rect(&self) -> (Rect, Size) {
        (self.rect, self.bounds)
    }

    pub fn set_scale120(&mut self, scale120: u32) {
        self.scale120 = scale120;
    }

    /// Renders now unless a frame callback is outstanding, in which case it is deferred to `frame_done`.
    pub fn request_render(&mut self, egl: &Egl, player: &Player, qh: &QueueHandle<crate::App>) {
        let stale = self.frame_pending.is_some_and(|t| t.elapsed() > FRAME_TIMEOUT);
        if self.configured && (self.frame_pending.is_none() || stale) {
            self.render(egl, player, qh);
        } else {
            self.dirty = true;
        }
    }

    pub fn frame_done(&mut self, egl: &Egl, player: &Player, qh: &QueueHandle<crate::App>) {
        self.frame_pending = None;
        if self.dirty {
            self.render(egl, player, qh);
        }
    }

    fn render(&mut self, egl: &Egl, player: &Player, qh: &QueueHandle<crate::App>) {
        let (w, h) = self.buffer_size();
        if stats::time(Call::MakeCurrent, || egl.make_current(Some(self.surface))).is_err() {
            return;
        }
        stats::count(Counter::Renders);
        stats::time(Call::Render, || player.render(w, h));
        self.wl_surface().frame(qh, FrameCallbackData(self.wl_surface().clone()));
        if let Err(e) = stats::time(Call::Swap, || egl.swap(self.surface)) {
            log::warn!("eglSwapBuffers: {e}");
        }
        player.report_swap();
        self.frame_pending = Some(Instant::now());
        self.dirty = false;
    }

    /// Tears down in protocol order: EGL surface, wl_egl_window, helper objects, then the layer surface.
    pub fn destroy(self, egl: &Egl) {
        egl.destroy_surface(self.surface);
        drop(self.window);
        if let Some(v) = self.viewport {
            v.destroy();
        }
        if let Some(f) = self.fractional {
            f.destroy();
        }
        if let Some(a) = self.alpha {
            a.destroy();
        }
    }
}
