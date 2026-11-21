#!/usr/bin/env bash
# Builds AniWay and installs it for the current user.
#
#   ./install.sh                 build, install into ~/.local and start AniWay
#   ./install.sh --prefix DIR    install into DIR instead (bin/, share/)
#   ./install.sh --no-start      install without starting the daemon and the editor
#   ./install.sh --uninstall     remove what an install put there (settings and scenes in ~/.config/aniway stay)
#   ./install.sh --purge         uninstall and also delete the settings, scenes and cache
#
# System packages are never installed from here: missing ones are listed with the command for your distribution.
set -euo pipefail
cd "$(dirname "$0")"

APP_ID=io.github.axl_std.AniWay
PREFIX=${PREFIX:-$HOME/.local}
UNINSTALL=false
PURGE=false
START=true
while [ $# -gt 0 ]; do
  case $1 in
    --prefix) PREFIX=${2:?--prefix needs a directory}; shift 2 ;;
    --uninstall) UNINSTALL=true; shift ;;
    --purge) UNINSTALL=true; PURGE=true; shift ;;
    --no-start) START=false; shift ;;
    -h | --help) sed -n '2,10p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "unknown option: $1 (see --help)" >&2; exit 2 ;;
  esac
done

BINS=(aniway aniwayd aniway-editor)
ICONS=$PREFIX/share/icons/hicolor/scalable/apps
ICON=$ICONS/$APP_ID.svg
# Tray icons under their own names: some panels (Quickshell) ignore the theme path the tray item sends.
TRAY_ICONS=(aniway-tray-light aniway-tray-dark)
DESKTOP=$PREFIX/share/applications/$APP_ID.desktop
AW=$PREFIX/bin/aniway
# Shell startup files that get the PATH line when $PREFIX/bin is missing from PATH; the marker finds it again.
PATH_MARK="# added by the AniWay installer"
RCS=("$HOME/.bashrc" "${ZDOTDIR:-$HOME}/.zshrc" "${XDG_CONFIG_HOME:-$HOME/.config}/fish/conf.d/aniway.fish")

say() { printf '\033[1m%s\033[0m\n' "$*"; }

# Launchers and panels watch these directories; touching them makes running ones pick up the entry and icons now.
refresh_caches() {
  command -v update-desktop-database >/dev/null && update-desktop-database -q "$PREFIX/share/applications" 2>/dev/null
  command -v gtk-update-icon-cache >/dev/null && gtk-update-icon-cache -q -f -t "$PREFIX/share/icons/hicolor" 2>/dev/null
  for d in "$PREFIX/share/icons/hicolor" "$PREFIX/share/applications"; do [ -d "$d" ] && touch "$d"; done
  for k in kbuildsycoca6 kbuildsycoca5; do command -v $k >/dev/null && { $k --noincremental >/dev/null 2>&1; break; }; done
  true
}

running() { [ -x "$AW" ] && "$AW" list >/dev/null 2>&1; }

stop_daemon() {
  running || return 0
  "$AW" quit >/dev/null 2>&1 || true
  for _ in $(seq 50); do running || return 0; sleep 0.2; done
}

# Appends the PATH line to the startup file of the login shell (bash, zsh or fish) and prints that file.
add_to_path() {
  case ":$PATH:" in *":$PREFIX/bin:"*) return 1 ;; esac
  local rc line
  case ${SHELL##*/} in
    zsh) rc=${RCS[1]} line="export PATH=\"$PREFIX/bin:\$PATH\"" ;;
    fish) rc=${RCS[2]} line="fish_add_path -g \"$PREFIX/bin\"" ;;
    *) rc=${RCS[0]} line="export PATH=\"$PREFIX/bin:\$PATH\"" ;;
  esac
  grep -qsF "$PATH_MARK" "$rc" && return 1
  mkdir -p "$(dirname "$rc")"
  printf '%s %s\n' "$line" "$PATH_MARK" >>"$rc"
  echo "$rc"
}

if $UNINSTALL; then
  stop_daemon
  [ -x "$AW" ] && "$AW" autostart off >/dev/null 2>&1
  for b in "${BINS[@]}"; do rm -fv "$PREFIX/bin/$b"; done
  rm -fv "$PREFIX"/share/locale/*/LC_MESSAGES/aniway.mo "$ICON" "$DESKTOP"
  for i in "${TRAY_ICONS[@]}"; do rm -fv "$ICONS/$i.svg"; done
  for rc in "${RCS[@]}"; do [ -f "$rc" ] && sed -i "\|$PATH_MARK\$|d" "$rc"; done
  rm -f "${RCS[2]}"
  refresh_caches
  if $PURGE; then
    rm -rf "${XDG_CONFIG_HOME:-$HOME/.config}/aniway" "${XDG_CACHE_HOME:-$HOME/.cache}/aniway"
    say "AniWay removed, with its settings, scenes and cache."
  else
    say "AniWay removed. Settings and scenes are still in ~/.config/aniway (./install.sh --purge deletes them)."
  fi
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
# A running daemon is the previous version: it stops before its files are replaced, and its editor closes with it.
stop_daemon
for b in "${BINS[@]}"; do install -Dm755 "target/release/$b" "$PREFIX/bin/$b"; done
for mo in target/locale/*/LC_MESSAGES/aniway.mo; do
  [ -e "$mo" ] || continue
  lang=$(basename "$(dirname "$(dirname "$mo")")")
  install -Dm644 "$mo" "$PREFIX/share/locale/$lang/LC_MESSAGES/aniway.mo"
done
install -Dm644 crates/aniway-editor/resources/icons/aniway-light.svg "$ICON"
for i in "${TRAY_ICONS[@]}"; do install -Dm644 "crates/aniway-editor/resources/icons/$i.svg" "$ICONS/$i.svg"; done
mkdir -p "$(dirname "$DESKTOP")"
# The icon is a full path, so launchers show it without waiting for an icon theme refresh.
cat >"$DESKTOP" <<EOF
[Desktop Entry]
Type=Application
Name=AniWay
GenericName=Desktop overlays
Comment=Animated click-through stickers for your Wayland desktop
Exec=$PREFIX/bin/aniway-editor
Icon=$ICON
Terminal=false
Categories=Utility;GTK;
Keywords=overlay;sticker;gif;video;wallpaper;hyprland;wayland;
StartupWMClass=$APP_ID
EOF
refresh_caches

say "AniWay is installed."
if rc=$(add_to_path); then
  echo "Added $PREFIX/bin to your PATH in $rc; open a new terminal to use the aniway command."
fi
if $START && [ -n "${WAYLAND_DISPLAY:-}" ]; then
  setsid -f "$PREFIX/bin/aniwayd" >/dev/null 2>&1 </dev/null
  for _ in $(seq 50); do running && break; sleep 0.2; done
  setsid -f "$PREFIX/bin/aniway-editor" >/dev/null 2>&1 </dev/null
  say "AniWay is running: the editor is open and the tray icon controls it."
else
  echo "Start it: open \"AniWay\" from your launcher (its Start button runs the daemon)."
fi
cat <<EOF

Start with login:  aniway autostart on   (or the switch in the editor's preferences)
Update later:      git pull && ./install.sh
Remove:            ./install.sh --uninstall   (--purge also deletes settings and scenes)
EOF
