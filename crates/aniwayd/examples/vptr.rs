//! Test client: sends one left click at the current cursor position through wlr-virtual-pointer.
//! With `DX DY` arguments the button is held while the pointer moves by that many pixels, then released;
//! a third `HOLD_MS` argument keeps it pressed that long first (time to take a screenshot).
//! `scroll N` sends N wheel steps instead (negative scrolls up).
use anyhow::{Context, Result};
use smithay_client_toolkit::reexports::client::globals::{registry_queue_init, GlobalListContents};
use smithay_client_toolkit::reexports::client::protocol::wl_pointer::{self, ButtonState};
use smithay_client_toolkit::reexports::client::protocol::wl_registry::WlRegistry;
use smithay_client_toolkit::reexports::client::{Connection, Dispatch, Proxy, QueueHandle};
use smithay_client_toolkit::reexports::protocols_wlr::virtual_pointer::v1::client::{
    zwlr_virtual_pointer_manager_v1::ZwlrVirtualPointerManagerV1, zwlr_virtual_pointer_v1::ZwlrVirtualPointerV1,
};

const BTN_LEFT: u32 = 0x110;

struct Idle;

impl Dispatch<WlRegistry, GlobalListContents> for Idle {
    fn event(
        _: &mut Self,
        _: &WlRegistry,
        _: <WlRegistry as Proxy>::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}
impl Dispatch<ZwlrVirtualPointerManagerV1, ()> for Idle {
    fn event(
        _: &mut Self,
        _: &ZwlrVirtualPointerManagerV1,
        _: <ZwlrVirtualPointerManagerV1 as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}
impl Dispatch<ZwlrVirtualPointerV1, ()> for Idle {
    fn event(
        _: &mut Self,
        _: &ZwlrVirtualPointerV1,
        _: <ZwlrVirtualPointerV1 as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

fn main() -> Result<()> {
    let conn = Connection::connect_to_env().context("no Wayland connection")?;
    let (globals, mut queue) = registry_queue_init::<Idle>(&conn)?;
    let qh = queue.handle();
    let manager: ZwlrVirtualPointerManagerV1 = globals.bind(&qh, 1..=2, ()).context("no virtual pointer manager")?;
    let pointer = manager.create_virtual_pointer(None, &qh, ());
    let mut state = Idle;
    let delta: Vec<f64> = std::env::args().skip(1).filter_map(|a| a.parse().ok()).collect();
    // Real timestamps: repeated runs with fixed ones look like double clicks to the client under the pointer.
    let now = || std::time::UNIX_EPOCH.elapsed().map_or(0, |d| d.as_millis() as u32);
    // `scroll N`: N wheel steps (positive = down) instead of a click.
    if std::env::args().nth(1).as_deref() == Some("scroll") {
        for _ in 0..(delta.first().copied().unwrap_or(1.0).abs() as u32) {
            let step = delta.first().copied().unwrap_or(1.0).signum();
            pointer.axis_source(wl_pointer::AxisSource::Wheel);
            pointer.axis_discrete(now(), wl_pointer::Axis::VerticalScroll, 15.0 * step, step as i32);
            pointer.frame();
            queue.roundtrip(&mut state)?;
            std::thread::sleep(std::time::Duration::from_millis(30));
        }
        pointer.destroy();
        queue.roundtrip(&mut state)?;
        return Ok(());
    }
    pointer.button(now(), BTN_LEFT, ButtonState::Pressed);
    pointer.frame();
    queue.roundtrip(&mut state)?;
    if let [dx, dy, ..] = delta[..] {
        for _ in 0..20 {
            pointer.motion(now(), dx / 20.0, dy / 20.0);
            pointer.frame();
            queue.roundtrip(&mut state)?;
            std::thread::sleep(std::time::Duration::from_millis(15));
        }
    }
    if let [_, _, hold] = delta[..] {
        std::thread::sleep(std::time::Duration::from_millis(hold as u64));
    }
    pointer.button(now(), BTN_LEFT, ButtonState::Released);
    pointer.frame();
    queue.roundtrip(&mut state)?;
    pointer.destroy();
    queue.roundtrip(&mut state)?;
    Ok(())
}
