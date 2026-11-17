//! Test client: types keys through zwp_virtual_keyboard_v1. Arguments are chords such as `alt`, `down`,
//! `shift+right`, `ctrl+2`, `delete`, `tab`, `esc`; a chord is pressed and released before the next one.
//! `chord:MS` holds it for that many milliseconds (80 by default).
use anyhow::{Context, Result};
use smithay_client_toolkit::reexports::client::globals::{registry_queue_init, GlobalListContents};
use smithay_client_toolkit::reexports::client::protocol::{
    wl_keyboard::KeyState, wl_registry::WlRegistry, wl_seat::WlSeat,
};
use smithay_client_toolkit::reexports::client::{Connection, Dispatch, Proxy, QueueHandle};
use smithay_client_toolkit::reexports::protocols_misc::zwp_virtual_keyboard_v1::client::{
    zwp_virtual_keyboard_manager_v1::ZwpVirtualKeyboardManagerV1, zwp_virtual_keyboard_v1::ZwpVirtualKeyboardV1,
};
use std::io::{Seek, Write};
use std::os::fd::AsFd;

const KEYMAP: &str = "xkb_keymap {
  xkb_keycodes { include \"evdev+aliases(qwerty)\" };
  xkb_types { include \"complete\" };
  xkb_compat { include \"complete\" };
  xkb_symbols { include \"pc+us+inet(evdev)\" };
};\0";

/// (evdev code, modifier bit) for each token.
fn key(name: &str) -> Option<(u32, u32)> {
    Some(match name {
        "shift" => (42, 1),
        "ctrl" => (29, 4),
        "alt" => (56, 8),
        "tab" => (15, 0),
        "comma" => (51, 0),
        "return" => (28, 0),
        "slash" => (53, 0),
        "esc" => (1, 0),
        "delete" => (111, 0),
        "left" => (105, 0),
        "up" => (103, 0),
        "right" => (106, 0),
        "down" => (108, 0),
        d if d.len() == 1 && d.as_bytes()[0].is_ascii_digit() => ((d.as_bytes()[0] - b'1') as u32 + 2, 0),
        _ => return None,
    })
}

struct Idle;
macro_rules! ignore {
    ($($t:ty => $u:ty),*) => {$(
        impl Dispatch<$t, $u> for Idle {
            fn event(_: &mut Self, _: &$t, _: <$t as Proxy>::Event, _: &$u, _: &Connection, _: &QueueHandle<Self>) {}
        }
    )*};
}
ignore!(WlRegistry => GlobalListContents, WlSeat => (), ZwpVirtualKeyboardManagerV1 => (), ZwpVirtualKeyboardV1 => ());

fn main() -> Result<()> {
    let conn = Connection::connect_to_env().context("no Wayland connection")?;
    let (globals, mut queue) = registry_queue_init::<Idle>(&conn)?;
    let qh = queue.handle();
    let seat: WlSeat = globals.bind(&qh, 1..=1, ())?;
    let manager: ZwpVirtualKeyboardManagerV1 = globals.bind(&qh, 1..=1, ()).context("no virtual keyboard manager")?;
    let keyboard = manager.create_virtual_keyboard(&seat, &qh, ());
    let mut file = tempfile_in_runtime()?;
    file.write_all(KEYMAP.as_bytes())?;
    file.rewind()?;
    keyboard.keymap(1, file.as_fd(), KEYMAP.len() as u32);
    let mut state = Idle;
    queue.roundtrip(&mut state)?;
    let mut time = 100;
    for arg in std::env::args().skip(1) {
        let (chord, hold) = arg.split_once(':').map_or((&arg[..], 80), |(c, ms)| (c, ms.parse().unwrap_or(80)));
        let keys: Vec<(u32, u32)> =
            chord.split('+').map(|k| key(k).with_context(|| format!("unknown key {k}"))).collect::<Result<_>>()?;
        let mods = keys.iter().fold(0, |m, k| m | k.1);
        keyboard.modifiers(mods, 0, 0, 0);
        for (code, _) in &keys {
            keyboard.key(time, *code, KeyState::Pressed.into());
            time += 20;
        }
        queue.roundtrip(&mut state)?;
        std::thread::sleep(std::time::Duration::from_millis(hold));
        for (code, _) in keys.iter().rev() {
            keyboard.key(time, *code, KeyState::Released.into());
            time += 20;
        }
        keyboard.modifiers(0, 0, 0, 0);
        queue.roundtrip(&mut state)?;
        std::thread::sleep(std::time::Duration::from_millis(120));
    }
    keyboard.destroy();
    queue.roundtrip(&mut state)?;
    Ok(())
}

fn tempfile_in_runtime() -> Result<std::fs::File> {
    let dir = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".into());
    let path = format!("{dir}/vkey-{}.xkb", std::process::id());
    let file = std::fs::OpenOptions::new().read(true).write(true).create_new(true).open(&path)?;
    std::fs::remove_file(&path)?;
    Ok(file)
}
