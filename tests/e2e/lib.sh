# Harness shared by run.sh: private nested Hyprland, daemon control, captures and assertions.
# Everything runs inside the nested compositor and a private XDG_CONFIG_HOME; the user session is never touched.

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
BIN=$ROOT/target/release
FIX=$ROOT/tests/fixtures
PIX="python3 -I $ROOT/tests/e2e/pix.py"
OW=$BIN/aniway
FAILS=0
CRIT_FAILED=0

log() { printf '%s\n' "$*"; }

assert() { # description, shell condition
  local desc=$1
  shift
  if eval "$@"; then log "  ok   $desc"; else log "  FAIL $desc"; FAILS=$((FAILS + 1)); fi
}

wait_until() { # seconds, shell condition
  local end=$((SECONDS + $1))
  shift
  while ! eval "$@" 2>/dev/null; do
    [ $SECONDS -ge $end ] && return 1
    sleep 0.2
  done
}

# Field of one overlay from `list --json` (jq expression applied to the overlay object).
jo() { "$OW" list --json | jq -r --arg id "$1" ".[] | select(.id == \$id) | $2"; }

# Value of key=value in the output of pix.py.
kv() { tr ' ' '\n' <<<"$1" | sed -n "s/^$2=//p"; }

preflight() {
  pgrep -x aniwayd >/dev/null && { log "aniwayd is already running; refusing to run"; exit 2; }
  for tool in grim jq hyprctl python3 mpv Hyprland; do
    command -v $tool >/dev/null || { log "missing tool: $tool"; exit 2; }
  done
  T=$(mktemp -d)
  export XDG_CONFIG_HOME=$T/cfg XDG_CACHE_HOME=$T/cache
  unset HYPRLAND_INSTANCE_SIGNATURE
}

write_lua() { # mode and transform of the secondary output
  cat >"$T/hypr.lua" <<LUA
hl.monitor({ output = "", mode = "preferred", position = "0x0", scale = "1" })
hl.monitor({ output = "OWTEST", mode = "$1", position = "2000x0", scale = "${OWSCALE:-1.25}", transform = ${2:-0} })
hl.config({
  general = { gaps_in = 0, gaps_out = 0, border_size = 0 },
  animations = { enabled = false },
  misc = { disable_hyprland_logo = true, disable_splash_rendering = true, force_default_wallpaper = 0, background_color = 0xff0000ff },
})
LUA
}

nested_start() {
  local before_sock before_sig
  before_sock=$(ls "$XDG_RUNTIME_DIR" | grep -E '^wayland-[0-9]+$')
  before_sig=$(ls "$XDG_RUNTIME_DIR/hypr")
  write_lua 1280x800@60
  Hyprland -c "$T/hypr.lua" >"$T/hypr.log" 2>&1 &
  HPID=$!
  local sock sig
  wait_until 20 'sock=$(comm -13 <(echo "$before_sock") <(ls "$XDG_RUNTIME_DIR" | grep -E "^wayland-[0-9]+$") | head -1); [ -n "$sock" ]' || return 1
  sig=$(comm -13 <(echo "$before_sig") <(ls "$XDG_RUNTIME_DIR/hypr") | head -1)
  export WAYLAND_DISPLAY=$sock HYPRLAND_INSTANCE_SIGNATURE=$sig
  wait_until 20 'hyprctl monitors -j | jq -e "length > 0" >/dev/null' || return 1
  MAIN=$(hyprctl monitors -j | jq -r '.[0].name')
}

nested_stop() {
  [ -n "${HPID:-}" ] && kill "$HPID" 2>/dev/null && wait "$HPID" 2>/dev/null
}

owtest_create() { write_lua 1280x800@60 "${TRANSFORM:-0}" && hyprctl reload >/dev/null && hyprctl output create headless OWTEST >/dev/null && wait_until 10 'hyprctl monitors -j | jq -e "any(.[]; .name == \"OWTEST\")" >/dev/null'; }
owtest_remove() { hyprctl output remove OWTEST >/dev/null; wait_until 10 '! hyprctl monitors -j | jq -e "any(.[]; .name == \"OWTEST\")" >/dev/null'; }

daemon_start() {
  RUST_LOG=${RUST_LOG:-info} "$OW" daemon >"$T/daemon.log" 2>&1 &
  wait_until 10 '"$OW" list >/dev/null'
}

daemon_stop() {
  "$OW" quit >/dev/null 2>&1
  wait_until 10 '! pgrep -x aniwayd >/dev/null'
}

fresh() { # [owtest]
  daemon_stop
  pkill -x mpv 2>/dev/null
  owtest_remove 2>/dev/null
  rm -rf "$XDG_CONFIG_HOME"
  mkdir -p "$XDG_CONFIG_HOME/aniway"
  [ "${1:-}" = owtest ] && owtest_create
  daemon_start
}

pss() { awk '/^Pss:/ { printf "%.0f", $2 / 1024 }' /proc/$(pgrep -x aniwayd)/smaps_rollup; }

shown() { wait_until "${2:-10}" "[ \"\$(jo $1 .shown)\" = true ]"; }

# Capture of an output at an explicit scale; prints the file path.
snap() { # name output scale
  sleep 0.6
  grim -s "$3" -t ppm -o "$2" "$T/$1.ppm" && echo "$T/$1.ppm"
}

# Region around an overlay rect (logical -> capture pixels) with a small margin.
region() { # rect-json scale
  jq -r --argjson s "$2" '[(.x * $s - 8 | floor), (.y * $s - 8 | floor), (.w * $s + 16 | ceil), (.h * $s + 16 | ceil)] | join(" ")' <<<"$1"
}

# Measures the daemon over `seconds`; sets CPU_PCT (percent of one core) and prints it with the RSS.
cpu_rss() { # label seconds
  local pid hz c0 c1
  pid=$(pgrep -x aniwayd)
  hz=$(getconf CLK_TCK)
  c0=$(awk '{print $14 + $15}' /proc/$pid/stat)
  sleep "$2"
  c1=$(awk '{print $14 + $15}' /proc/$pid/stat)
  CPU_PCT=$(awk -v a="$c0" -v b="$c1" -v hz="$hz" -v s="$2" 'BEGIN { printf "%.0f", (b - a) * 100 / hz / s }')
  printf '  info %s: CPU %s%%, RSS %s MB\n' "$1" "$CPU_PCT" "$(awk '/VmRSS/ { printf "%.0f", $2 / 1024 }' /proc/$pid/status)"
}

# Window that records left clicks in $T/clicked; it fills the output, so the overlay sits on top of it.
click_target() {
  local bindings=""
  for mod in "" Meta+ Ctrl+ Alt+ Shift+; do bindings+="${mod}MBTN_LEFT run touch $T/clicked"$'\n'; done
  printf '%s' "$bindings" >"$T/input.conf"
  rm -f "$T/clicked"
  mpv --no-config --idle=yes --force-window=yes --input-default-bindings=no --input-conf="$T/input.conf" >/dev/null 2>&1 &
  wait_until 10 'hyprctl clients -j | jq -e "any(.[]; .class == \"mpv\")" >/dev/null'
  sleep 0.5
}

# The editor runs on a private session bus without the desktop portal, so it can never reach the real desktop
# (file dialogs, settings); EDITOR_ENV may hold extra VAR=value assignments (renderer, colour scheme).
editor_start() {
  dbus-run-session -- env GTK_USE_PORTAL=0 GDK_DEBUG=no-portals GTK_A11Y=none $EDITOR_ENV "$BIN/aniway-editor" >"$T/editor.log" 2>&1 &
  EPID=$!
}

editor_stop() {
  [ -n "${EPID:-}" ] || return 0
  pkill -TERM -P "$EPID" 2>/dev/null
  kill "$EPID" 2>/dev/null
  wait "$EPID" 2>/dev/null
  EPID=
}

run_criterion() { # number, description, function
  FAILS=0
  log "== $1. $2"
  "$3"
  if [ $FAILS -eq 0 ]; then log "PASS $1"; else log "FAIL $1 ($FAILS assertion(s))"; CRIT_FAILED=$((CRIT_FAILED + 1)); fi
}
