<img width="1280" height="640" alt="AniWay" src="https://github.com/user-attachments/assets/e3874a00-f9c1-4ce4-8c07-f7459760e4f7" />

Animated stickers for your Wayland desktop. Videos, GIFs, APNG, WebP and images become click-through overlays on any
of your monitors. Place them on a map of your screens, group them in scenes with their own shortcuts, and choose for
each one whether it stays above fullscreen apps or under your windows.

A small background service (`aniwayd`) draws them with libmpv and hardware decoding; the editor (`aniway-editor`) and
the command line (`aniway`) only control it, so the overlays keep running with the editor closed.

## Quick start

```bash
git clone https://github.com/Axl-STD/AniWay.git && cd AniWay && ./install.sh
```

The script checks the dependencies (and tells you the command to install any that are missing), builds AniWay and
installs it into `~/.local`, with a launcher entry. Then:

```bash
aniway daemon &        # start the service
aniway-editor          # or open "AniWay" from your launcher
```

To start it with your session: `aniway autostart on`. To update: `git pull && ./install.sh`, then `aniway quit` and
start it again. To remove it: `./install.sh --uninstall`.

## Features

- Anything mpv can open: video (with transparency too), GIF, APNG, animated WebP and still images.
- Editor with a map of your monitors: drag, anchor to 9 points, scale, opacity, speed and layer
  (above everything, above windows, below windows, desktop background).
- Overflow: lets up to half of an overlay go past the monitor edge, to hide transparent margins of the media.
- Scenes stored as TOML, each with its own keyboard shortcut on Hyprland.
- Optimized copies of media that is drawn much smaller than its size, and a per-overlay frame rate limit.
- Tray icon, pause or hide everything for a while, optional start at login.
- Seven themes; English and Spanish interface.

## Compatibility

| Compositor | Status |
|---|---|
| Hyprland | Tested on 0.57. Every feature. |
| Other compositors with `wlr-layer-shell` (Sway, river, Wayfire, labwc, niri, KDE Plasma, COSMIC) | Should work, untested. No scene shortcuts or window-class rules. |
| GNOME, X11 | Not supported (no layer-shell). |

## Building by hand

Requirements: Rust (stable), libmpv 2.0+, GTK 4.14+, libadwaita 1.8+, gettext and ffmpeg (`ffprobe` for media
information and thumbnails, `ffmpeg` for optimized copies).

```bash
cargo build --release --workspace --bins
target/release/aniway daemon &
target/release/aniway-editor
```

## Command line

```bash
aniway add ~/sticker.gif --anchor bottom-right --scale 0.5
aniway list
aniway scene load night
aniway --help
```

Settings and scenes live in `~/.config/aniway/`.

## License

[GPL-3.0](LICENSE).
