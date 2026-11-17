#!/usr/bin/env bash
# Builds AniWay and installs it for the current user.
#
#   ./install.sh                 build and install into ~/.local
#   ./install.sh --prefix DIR    install into DIR instead (bin/, share/)
#   ./install.sh --uninstall     remove what an install put there (settings and scenes in ~/.config/aniway stay)
#
# System packages are never installed from here: missing ones are listed with the command for your distribution.
set -euo pipefail
cd "$(dirname "$0")"

APP_ID=io.github.axl_std.AniWay
PREFIX=${PREFIX:-$HOME/.local}
UNINSTALL=false
while [ $# -gt 0 ]; do
  case $1 in
    --prefix) PREFIX=${2:?--prefix needs a directory}; shift 2 ;;
    --uninstall) UNINSTALL=true; shift ;;
    -h | --help) sed -n '2,8p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "unknown option: $1 (see --help)" >&2; exit 2 ;;
  esac
done

BINS=(aniway aniwayd aniway-editor)
ICON=$PREFIX/share/icons/hicolor/scalable/apps/$APP_ID.svg
DESKTOP=$PREFIX/share/applications/$APP_ID.desktop

say() { printf '\033[1m%s\033[0m\n' "$*"; }

refresh_caches() {
  command -v update-desktop-database >/dev/null && update-desktop-database -q "$PREFIX/share/applications" 2>/dev/null
  command -v gtk-update-icon-cache >/dev/null && gtk-update-icon-cache -q -t "$PREFIX/share/icons/hicolor" 2>/dev/null
  true
}

if $UNINSTALL; then
  for b in "${BINS[@]}"; do rm -fv "$PREFIX/bin/$b"; done
  rm -fv "$PREFIX"/share/locale/*/LC_MESSAGES/aniway.mo "$ICON" "$DESKTOP"
  refresh_caches
  say "AniWay removed. Settings and scenes are still in ~/.config/aniway."
  exit 0
fi

# Everything the build and the app need, as "check|what is missing".
missing=()
command -v cargo >/dev/null || missing+=("Rust (cargo)")
command -v pkg-config >/dev/null || missing+=("pkg-config")
if command -v pkg-config >/dev/null; then
  pkg-config --atleast-version=2.0 mpv || missing+=("libmpv 2.0+ (development files)")
  pkg-config --atleast-version=4.14 gtk4 || missing+=("GTK 4.14+ (development files)")
  pkg-config --atleast-version=1.8 libadwaita-1 || missing+=("libadwaita 1.8+ (development files)")
fi
command -v msgfmt >/dev/null || missing+=("gettext (msgfmt, for the translations)")
command -v ffprobe >/dev/null || missing+=("ffmpeg (ffprobe and ffmpeg, for media info and optimized copies)")

if [ ${#missing[@]} -gt 0 ]; then
  say "Missing:"
  printf '  - %s\n' "${missing[@]}"
  . /etc/os-release 2>/dev/null || true
  case " ${ID:-} ${ID_LIKE:-} " in
    *" arch "*) echo "Install them with: sudo pacman -S --needed rust pkgconf mpv gtk4 libadwaita gettext ffmpeg" ;;
    *" fedora "*) echo "Install them with: sudo dnf install cargo pkgconf mpv-devel gtk4-devel libadwaita-devel gettext ffmpeg" ;;
    *" debian "* | *" ubuntu "*) echo "Install them with: sudo apt install cargo pkg-config libmpv-dev libgtk-4-dev libadwaita-1-dev gettext ffmpeg" ;;
    *" opensuse "* | *" suse "*) echo "Install them with: sudo zypper install cargo pkgconf mpv-devel gtk4-devel libadwaita-devel gettext-tools ffmpeg" ;;
  esac
  echo "If your distribution's Rust is too old, get the current one from https://rustup.rs"
  exit 1
fi

say "Building (the first build takes a few minutes)…"
cargo build --release --locked --workspace --bins

say "Installing into $PREFIX…"
for b in "${BINS[@]}"; do install -Dm755 "target/release/$b" "$PREFIX/bin/$b"; done
for mo in target/locale/*/LC_MESSAGES/aniway.mo; do
  [ -e "$mo" ] || continue
  lang=$(basename "$(dirname "$(dirname "$mo")")")
  install -Dm644 "$mo" "$PREFIX/share/locale/$lang/LC_MESSAGES/aniway.mo"
done
install -Dm644 crates/aniway-editor/resources/icons/aniway-light.svg "$ICON"
mkdir -p "$(dirname "$DESKTOP")"
cat >"$DESKTOP" <<EOF
[Desktop Entry]
Type=Application
Name=AniWay
GenericName=Desktop overlays
Comment=Animated click-through stickers for your Wayland desktop
Exec=$PREFIX/bin/aniway-editor
Icon=$APP_ID
Terminal=false
Categories=Utility;GTK;
Keywords=overlay;sticker;gif;video;wallpaper;hyprland;wayland;
StartupWMClass=$APP_ID
EOF
refresh_caches

say "AniWay is installed."
case ":$PATH:" in
  *":$PREFIX/bin:"*) ;;
  *) echo "Note: $PREFIX/bin is not in your PATH; add it to use the commands by name." ;;
esac
cat <<EOF

Start it:          aniway daemon &   then open "AniWay" from your launcher (or run aniway-editor)
Start with login:  aniway autostart on   (or start aniwayd from your compositor's autostart)
Update later:      git pull && ./install.sh   (then: aniway quit, and start it again)
EOF
