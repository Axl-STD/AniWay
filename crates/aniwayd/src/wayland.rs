use crate::App;
use anyhow::{Context, Result};
use smithay_client_toolkit::compositor::{CompositorHandler, CompositorState};
use smithay_client_toolkit::dispatch2::Dispatch2;
use smithay_client_toolkit::output::{OutputHandler, OutputInfo, OutputState};
use smithay_client_toolkit::reexports::client::globals::GlobalList;
use smithay_client_toolkit::reexports::client::protocol::{wl_output, wl_surface};
use smithay_client_toolkit::reexports::client::{Connection, Proxy, QueueHandle};
use smithay_client_toolkit::reexports::protocols::wp::alpha_modifier::v1::client::{
    wp_alpha_modifier_surface_v1::WpAlphaModifierSurfaceV1, wp_alpha_modifier_v1::WpAlphaModifierV1,
};
use smithay_client_toolkit::reexports::protocols::wp::fractional_scale::v1::client::{
    wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1,
    wp_fractional_scale_v1::{Event as FractionalEvent, WpFractionalScaleV1},
};
use smithay_client_toolkit::reexports::protocols::wp::viewporter::client::{
    wp_viewport::WpViewport, wp_viewporter::WpViewporter,
};
use smithay_client_toolkit::registry::{ProvidesRegistryState, RegistryState};
use smithay_client_toolkit::shell::wlr_layer::{LayerShell, LayerShellHandler, LayerSurface, LayerSurfaceConfigure};
use smithay_client_toolkit::{delegate_dispatch2, delegate_registry, registry_handlers};

/// Wayland globals the daemon uses.
pub struct Wl {
    pub conn: Connection,
    pub qh: QueueHandle<App>,
    pub registry: RegistryState,
    pub outputs: OutputState,
    pub compositor: CompositorState,
    pub layer_shell: LayerShell,
    pub viewporter: Option<WpViewporter>,
    pub fractional: Option<WpFractionalScaleManagerV1>,
    pub alpha: Option<WpAlphaModifierV1>,
}

/// User data for protocol objects whose events are ignored.
pub struct Noop;

/// User data for `wp_fractional_scale_v1`: the surface it belongs to.
pub struct FractionalData(pub wl_surface::WlSurface);

impl Wl {
    pub fn bind(conn: Connection, globals: &GlobalList, qh: QueueHandle<App>) -> Result<Self> {
        let viewporter = globals.bind(&qh, 1..=1, Noop).ok();
        let fractional = globals.bind(&qh, 1..=1, Noop).ok();
        let alpha = globals.bind(&qh, 1..=1, Noop).ok();
        if viewporter.is_none() || fractional.is_none() {
            log::warn!("wp_viewporter/wp_fractional_scale missing: using integer output scale");
        }
        if alpha.is_none() {
            log::warn!("wp_alpha_modifier_v1 missing: overlay opacity is ignored");
        }
        Ok(Self {
            registry: RegistryState::new(globals),
            outputs: OutputState::new(globals, &qh),
            compositor: CompositorState::bind(globals, &qh).context("wl_compositor missing")?,
            layer_shell: LayerShell::bind(globals, &qh)
                .context("zwlr_layer_shell_v1 missing: not a wlroots-style compositor")?,
            viewporter,
            fractional,
            alpha,
            conn,
            qh,
        })
    }

    pub fn display_ptr(&self) -> *mut std::ffi::c_void {
        self.conn.backend().display_ptr() as *mut _
    }

    pub fn output_infos(&self) -> Vec<(wl_output::WlOutput, OutputInfo)> {
        self.outputs.outputs().filter_map(|o| self.outputs.info(&o).map(|i| (o, i))).collect()
    }

    /// Resolves a connector name or `desc:<description prefix>`; `None` selects the first output by name.
    pub fn find_output(&self, want: Option<&str>) -> Option<(wl_output::WlOutput, OutputInfo)> {
        let mut outputs = self.output_infos();
        outputs.sort_by(|a, b| a.1.name.cmp(&b.1.name));
        outputs.into_iter().find(|(_, i)| match want {
            None => true,
            Some(d) => match d.strip_prefix("desc:") {
                Some(prefix) => i.description.as_deref().is_some_and(|desc| desc.starts_with(prefix.trim())),
                None => i.name.as_deref() == Some(d),
            },
        })
    }
}

impl CompositorHandler for App {
    fn scale_factor_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        surface: &wl_surface::WlSurface,
        factor: i32,
    ) {
        self.surface_scale(surface, factor.max(1) as u32 * 120, false);
    }

    fn transform_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: wl_output::Transform,
    ) {
    }

    fn frame(&mut self, _: &Connection, _: &QueueHandle<Self>, surface: &wl_surface::WlSurface, _: u32) {
        self.frame_done(surface);
    }

    fn surface_enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }

    fn surface_leave(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }
}

impl OutputHandler for App {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.wl.outputs
    }

    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {
        self.reconcile();
    }

    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {
        self.reconcile();
    }

    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {
        self.reconcile();
    }
}

impl LayerShellHandler for App {
    fn closed(&mut self, _: &Connection, _: &QueueHandle<Self>, layer: &LayerSurface) {
        self.layer_closed(layer);
    }

    fn configure(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        layer: &LayerSurface,
        _: LayerSurfaceConfigure,
        _: u32,
    ) {
        self.layer_configured(layer);
    }
}

impl ProvidesRegistryState for App {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.wl.registry
    }
    registry_handlers![OutputState];
}

impl Dispatch2<WpFractionalScaleV1, App> for FractionalData {
    fn event(
        &self,
        app: &mut App,
        _: &WpFractionalScaleV1,
        event: FractionalEvent,
        _: &Connection,
        _: &QueueHandle<App>,
    ) {
        let FractionalEvent::PreferredScale { scale } = event else {
            return;
        };
        app.surface_scale(&self.0, scale, true);
    }
}

macro_rules! ignore_events {
    ($($proxy:ty),*) => {$(
        impl Dispatch2<$proxy, App> for Noop {
            fn event(&self, _: &mut App, _: &$proxy, _: <$proxy as Proxy>::Event, _: &Connection, _: &QueueHandle<App>) {}
        }
    )*};
}
ignore_events!(WpViewporter, WpViewport, WpFractionalScaleManagerV1, WpAlphaModifierV1, WpAlphaModifierSurfaceV1);

delegate_registry!(App);
delegate_dispatch2!(App);
