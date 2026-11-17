#!/usr/bin/env bash
# End-to-end acceptance suite. Usage: tests/e2e/run.sh [criterion numbers...]
# Needs release binaries, fixtures (scripts/make-fixtures.sh) and tools: Hyprland, hyprctl, grim, jq, mpv, python3.
set -uo pipefail
cd "$(dirname "$0")/../.."
. tests/e2e/lib.sh

c1() {
  assert "cargo fmt --check" 'cargo fmt --check >"$T/build.log" 2>&1'
  assert "clippy -D warnings" 'cargo clippy --workspace --all-targets -- -D warnings >>"$T/build.log" 2>&1'
  assert "tests" 'cargo test --workspace >>"$T/build.log" 2>&1'
  touch crates/*/src/main.rs
  assert "release build without warnings" 'cargo build --release --workspace --bins --examples >"$T/release.log" 2>&1 && ! grep -q "^warning" "$T/release.log"'
}

c2() {
  fresh
  local out rc
  out=$("$OW" daemon 2>&1)
  rc=$?
  assert "second instance exits with code 3" '[ $rc -eq 3 ]'
  assert "message says already running" 'grep -qi "already running" <<<"$out"'
  pkill -TERM -x aniwayd
  assert "SIGTERM stops the daemon" 'wait_until 5 "! pgrep -x aniwayd >/dev/null"'
  assert "socket removed" '[ ! -e "$XDG_RUNTIME_DIR/aniway.sock" ]'
  assert "no errors logged" '! grep -q "ERROR" "$T/daemon.log"'
}

# name file extra-args kind(opaque|alpha)
check_fixture() {
  local name=$1 file=$2 kind=$4 rect reg bb w h
  "$OW" add "$FIX/$file" --id "$name" --pos 100,100 $3 >/dev/null
  shown "$name" || { assert "$file shown" false; return; }
  rect=$(jo "$name" .rect | jq -c .)
  reg=$(region "$rect" 1)
  bb=$($PIX bbox "$(snap "$name" $MAIN 1)" $reg)
  w=$(kv "$bb" w) h=$(kv "$bb" h)
  local rw rh
  rw=$(jq .w <<<"$rect") rh=$(jq .h <<<"$rect")
  if [ "$kind" = opaque ]; then
    local mw
    mw=$(jo "$name" .media.w)
    assert "$file rect matches media size times scale" '[ "$(awk -v m=$mw -v s=$(jo "$name" .scale) "BEGIN { printf \"%d\", m * s + 0.5 }")" = $rw ]'
    assert "$file drawn at its size ${rw}x${rh} (got ${w}x${h})" '[ ${w#-} -ge $((rw - 1)) ] && [ $w -le $((rw + 1)) ] && [ $h -ge $((rh - 1)) ] && [ $h -le $((rh + 1)) ]'
  else
    assert "$file contained in ${rw}x${rh} and not collapsed (got ${w}x${h})" '[ $w -le $rw ] && [ $h -le $rh ] && [ $w -ge $(( (rw < rh ? rw : rh) * 7 / 10)) ] && [ $h -ge $(( (rw < rh ? rw : rh) * 7 / 10)) ]'
    local hs
    hs=$($PIX halo "$T/$name.ppm" $reg)
    assert "$file has no halo ($hs)" '[ "$(kv "$hs" bad)" -eq 0 ]'
  fi
  "$OW" remove "$name" >/dev/null
}

c3() {
  fresh
  check_fixture png alpha.png "" alpha
  check_fixture gif anim.gif "" alpha
  check_fixture apng anim.apng "" alpha
  check_fixture webp anim.webp "" alpha
  check_fixture webm alpha.webm "" alpha
  check_fixture stripes stripes320.png "" opaque
  check_fixture h264 h264_1080p.mp4 "--scale 0.25" opaque
  check_fixture av1 av1_1080p.mp4 "--scale 0.25" opaque
  check_fixture vertical vertical.mp4 "--scale 0.5" opaque
  "$OW" add "$FIX/corrupt.png" --id bad --pos 100,100 >/dev/null
  assert "undecodable media reports an error instead of staying silent" 'wait_until 12 "[ \"\$(jo bad .error)\" = \"could not decode the media\" ]" && [ "$(jo bad .shown)" = false ]'
  "$OW" remove bad >/dev/null
  "$OW" add "$FIX/stripes320.png" --id s --pos 100,100 >/dev/null
  shown s
  local st
  st=$($PIX stripes "$(snap sharp $MAIN 1)" 100 100 320 320)
  assert "1-pixel stripes keep full contrast ($st)" '[ "$(awk -v r="$(kv "$st" ratio)" "BEGIN { print (r >= 0.95) }")" = 1 ]'
}

c4() {
  fresh
  "$OW" add "$FIX/hevc_4k_60s.mp4" --id hevc --scale 0.125 --pos 50,50 >/dev/null
  shown hevc
  assert "hwdec active ($(jo hevc .hwdec))" '[ "$(jo hevc .hwdec)" != no ] && [ "$(jo hevc .hwdec)" != null ]'
  local start=$SECONDS first rect reg
  rect=$(jo hevc .rect | jq -c .)
  reg=$(region "$rect" 1)
  first=$(snap hevc1 $MAIN 1)
  assert "frames advance" '[ "$($PIX diff "$(snap hevc2 $MAIN 1)" "$first" | cut -d= -f2)" -gt 0 ]'
  cpu_rss "HEVC 4K 60 s" 15
  sleep $((66 - (SECONDS - start)))
  local a b
  a=$(snap hevc3 $MAIN 1)
  sleep 0.5
  b=$(snap hevc4 $MAIN 1)
  assert "still playing after more than 60 s (looping)" '[ "$($PIX diff "$a" "$b" | cut -d= -f2)" -gt 0 ] && [ "$(jo hevc .error)" = null ]'
  "$OW" remove hevc >/dev/null
  for i in 1 2 3 4 5 6 7 8 9 10; do "$OW" add "$FIX/anim.gif" --id g$i --pos $((i * 60)),$((i * 50)) >/dev/null; done
  sleep 3
  cpu_rss "10 GIFs" 15
  assert "10 GIFs shown" '[ "$("$OW" list --json | jq "[.[] | select(.shown)] | length")" = 10 ]'
}

c5() {
  fresh
  click_target
  "$OW" add "$FIX/stripes320.png" --id top --pos 100,100 --layer overlay >/dev/null
  shown top
  hyprctl dispatch 'hl.dsp.cursor.move({ x = 250, y = 250 })' >/dev/null
  sleep 0.5
  "$ROOT/target/release/examples/vptr"
  assert "click crosses the overlay and reaches the window below" 'wait_until 3 "[ -e \"$T/clicked\" ]"'
}

c6() {
  fresh
  local scene=$XDG_CONFIG_HOME/aniway/scenes/default.toml
  "$OW" add "$FIX/anim.gif" --id p1 --pos 10,20 >/dev/null
  "$OW" add "$FIX/anim.gif" --id p2 >/dev/null
  "$OW" move p1 30,40 >/dev/null
  "$OW" set p1 --opacity 0.5 >/dev/null
  "$OW" remove p2 >/dev/null
  assert "list reflects add/set/move/remove" '[ "$(jo p1 .x)" = 30 ] && [ "$(jo p1 .opacity)" = 0.5 ] && [ -z "$(jo p2 .id)" ]'
  assert "scene file persisted" 'grep -q "id = \"p1\"" "$scene" && grep -q "x = 30" "$scene" && ! grep -q "p2" "$scene"'
  local reloads
  reloads=$(grep -c reloaded "$T/daemon.log")
  sleep 1.5
  assert "own writes cause no reload echo" '[ "$(grep -c reloaded "$T/daemon.log")" = "$reloads" ]'
  cp "$scene" "$T/good.toml"
  sed 's/^x = 30/x = 77/' "$scene" >"$T/edit.toml" && mv "$T/edit.toml" "$scene"
  assert "hand edit reloads live" 'wait_until 4 "[ \"\$(jo p1 .x)\" = 77 ]"'
  local sum
  sum=$(md5sum <"$scene")
  sleep 1.5
  assert "no write loop after the reload" '[ "$(md5sum <"$scene")" = "$sum" ] && [ "$(grep -c reloaded "$T/daemon.log")" = $((reloads + 1)) ]'
  printf '[[overlay\nid = ' >"$T/broken.toml" && mv "$T/broken.toml" "$scene"
  sleep 1.5
  assert "broken TOML keeps previous state" '[ "$(jo p1 .x)" = 77 ] && [ "$(jo p1 .shown)" = true ]'
  assert "broken TOML logs line and column" 'grep -Eq "line [0-9]+, column [0-9]+" "$T/daemon.log"'
}

fullscreen_window() { # monitor
  hyprctl dispatch "hl.dsp.window.move({ monitor = \"$1\" })" >/dev/null
  hyprctl dispatch 'hl.dsp.window.fullscreen({ mode = "fullscreen" })' >/dev/null
  sleep 1.2
}

c7() {
  fresh owtest
  "$OW" add "$FIX/anim.gif" --id a --output $MAIN --pos 50,50 >/dev/null
  "$OW" add "$FIX/anim.gif" --id b --output OWTEST --pos 50,50 --layer top >/dev/null
  shown a && shown b
  click_target
  fullscreen_window OWTEST
  assert "a fullscreen app hides and pauses nothing" '[ "$(jo b .shown)" = true ] && [ "$(jo b .paused)" = false ] && [ "$(jo a .shown)" = true ]'
  local reg
  reg=$(region "$(jo b .rect | jq -c .)" 1)
  assert "layer top stays under the fullscreen app" '[ "$(kv "$($PIX warm "$(snap fs-top OWTEST 1)" $reg)" n)" -lt 50 ]'
  "$OW" set b --layer overlay >/dev/null
  assert "layer overlay is drawn above the fullscreen app" '[ "$(kv "$($PIX warm "$(snap fs-overlay OWTEST 1)" $reg)" n)" -gt 1000 ]'
  "$OW" set b --layer top >/dev/null
  hyprctl dispatch 'hl.dsp.window.fullscreen({ mode = "fullscreen" })' >/dev/null
  assert "layer top shows again when fullscreen ends" '[ "$(kv "$($PIX warm "$(snap fs-end OWTEST 1)" $reg)" n)" -gt 1000 ]'
  printf 'scene = "default"\n' >"$XDG_CONFIG_HOME/aniway/config.toml"
  printf 'fullscreen_includes_maximized = true\n' >>"$XDG_CONFIG_HOME/aniway/config.toml"
  sed -i 's/^layer = .*/&\non_fullscreen = "hide"/' "$XDG_CONFIG_HOME/aniway/scenes/default.toml"
  "$OW" reload >/dev/null
  sleep 1.2
  assert "retired fullscreen keys load without warnings" '! grep -q "unknown key" "$T/daemon.log" && [ "$(jo b .shown)" = true ]'
  printf 'hide_for_classes = ["mpv"]\n' >"$XDG_CONFIG_HOME/aniway/config.toml"
  "$OW" reload >/dev/null
  sleep 1.2
  assert "hide_for_classes hides on the monitor with the class" '[ "$(jo b .shown)" = false ] && [ "$(jo a .shown)" = true ]'
}

c8() {
  fresh
  "$OW" add "$FIX/anim.gif" --id h --pos 50,50 >/dev/null
  shown h
  "$OW" hide --for 30s >/dev/null
  sleep 1
  assert "hide --for hides" '[ "$(jo h .shown)" = false ]'
  assert "overlay comes back by itself" 'wait_until 35 "[ \"\$(jo h .shown)\" = true ]"'
}

c9() {
  fresh owtest
  "$OW" add "$FIX/anim.gif" --id b --output OWTEST --pos 50,50 >/dev/null
  shown b
  owtest_remove
  sleep 1
  assert "overlay stays pending without its output" '[ "$(jo b .shown)" = false ] && [ "$(jo b .error)" = null ] && [ "$(jo b .visible)" = true ]'
  owtest_create
  assert "overlay restored when the output returns" 'shown b 10'
}

c10() {
  fresh owtest
  "$OW" add "$FIX/stripes400.png" --id s --output OWTEST --pos 100,100 --size 320x320 >/dev/null
  shown s
  local rect reg bb st
  rect=$(jo s .rect | jq -c .)
  assert "output scale reported as 1.25" '[ "$("$OW" outputs --json | jq -r ".[] | select(.name == \"OWTEST\") | .scale")" = 1.25 ]'
  assert "logical size is 320x320" '[ "$(jq -r "[.w, .h] | join(\"x\")" <<<"$rect")" = 320x320 ]'
  reg=$(region "$rect" 1.25)
  bb=$($PIX bbox "$(snap fractional OWTEST 1.25)" $reg)
  assert "drawn at 400x400 physical pixels (got $(kv "$bb" w)x$(kv "$bb" h))" '[ "$(kv "$bb" w)" = 400 ] && [ "$(kv "$bb" h)" = 400 ]'
  st=$($PIX stripes "$T/fractional.ppm" "$(kv "$bb" x)" "$(kv "$bb" y)" 400 400)
  assert "sharp at fractional scale ($st)" '[ "$(awk -v r="$(kv "$st" ratio)" "BEGIN { print (r >= 0.95) }")" = 1 ]'
}

c11() {
  fresh owtest
  "$OW" add "$FIX/stripes320.png" --id c --output OWTEST --anchor bottom-right --pos=-20,-20 --size 100x100 >/dev/null
  shown c
  local mode
  for mode in 1280x800 1000x600; do
    write_lua "$mode@60"
    hyprctl reload >/dev/null
    local want_w=$((${mode%x*} * 4 / 5)) want_h=$((${mode#*x} * 4 / 5))
    wait_until 10 "[ \"\$(\"$OW\" outputs --json | jq -r '.[] | select(.name == \"OWTEST\") | .rect.w')\" = $want_w ]"
    sleep 1
    local bb phys_w phys_h
    phys_w=${mode%x*} phys_h=${mode#*x}
    bb=$($PIX bbox "$(snap anchor OWTEST 1.25)" $((phys_w - 200)) $((phys_h - 200)) 200 200)
    local right=$(($(kv "$bb" x) + $(kv "$bb" w))) bottom=$(($(kv "$bb" y) + $(kv "$bb" h)))
    assert "$mode: stuck to the corner with a 20 px logical offset (right=$right of $phys_w, bottom=$bottom of $phys_h)" \
      '[ $((phys_w - right - 25)) -ge -2 ] && [ $((phys_w - right - 25)) -le 2 ] && [ $((phys_h - bottom - 25)) -ge -2 ] && [ $((phys_h - bottom - 25)) -le 2 ]'
  done
}

c12() {
  TRANSFORM=1 fresh owtest
  "$OW" add "$FIX/stripes320.png" --id c --output OWTEST --anchor bottom-right --pos=-20,-20 --size 100x100 >/dev/null
  "$OW" add "$FIX/stripes320.png" --id tl --output OWTEST --pos 100,100 --size 100x100 >/dev/null
  shown c && shown tl
  local out
  out=$("$OW" outputs --json | jq -c '.[] | select(.name == "OWTEST")')
  assert "logical size is rotated (640x1024, got $(jq -r '[.rect.w, .rect.h] | join("x")' <<<"$out"))" '[ "$(jq -r ".rect.w" <<<"$out")" = 640 ] && [ "$(jq -r ".rect.h" <<<"$out")" = 1024 ]'
  assert "scale is 1.25 on a rotated output (got $(jq .scale <<<"$out"))" '[ "$(jq .scale <<<"$out")" = 1.25 ]'
  local snapfile bb right bottom
  snapfile=$(snap rotated OWTEST 1.25)
  bb=$($PIX bbox "$snapfile" 100 100 150 150)
  assert "top-left overlay at 125,125 physical (got $(kv "$bb" x),$(kv "$bb" y))" '[ "$(kv "$bb" x)" = 125 ] && [ "$(kv "$bb" y)" = 125 ]'
  bb=$($PIX bbox "$snapfile" 600 1080 200 200)
  right=$(($(kv "$bb" x) + $(kv "$bb" w))) bottom=$(($(kv "$bb" y) + $(kv "$bb" h)))
  assert "bottom-right overlay stuck to the corner of the 800x1280 capture (right=$right, bottom=$bottom)" '[ $((800 - right - 25)) -ge -2 ] && [ $((800 - right - 25)) -le 2 ] && [ $((1280 - bottom - 25)) -ge -2 ] && [ $((1280 - bottom - 25)) -le 2 ]'
}

# N small videos over 2 monitors for 60 s: checks software decoding, memory, frame loss and CPU.
small_videos() { # count max-cpu-percent max-lost-percent
  local n=$1 max_cpu=$2 max_lost=$3 i out base dropped total
  fresh owtest
  for i in $(seq 1 "$n"); do
    out=$MAIN
    [ $((i % 2)) = 0 ] && out=OWTEST
    "$OW" add "$FIX/vertical15.mp4" --id v$i --output $out --scale 0.5 --pos $(((i % 8) * 100)),$(((i % 3) * 120)) >/dev/null
  done
  sleep 6
  base=$(pss)
  cpu_rss "$n small videos" 60
  dropped=$("$OW" list --json | jq '[.[].dropped] | add')
  total=$((n * 30 * 66))
  assert "$n videos shown and decoded in software" '[ "$("$OW" list --json | jq "[.[] | select(.shown and (.hwdec == null or .hwdec == \"no\"))] | length")" = $n ]'
  assert "$n videos: total Pss ${base} MB <= 600" '[ "$base" -le 600 ]'
  assert "$n videos: frames lost $dropped of ~$total, limit ${max_lost}%" 'if [ $max_lost = 0 ]; then [ "$dropped" = 0 ]; else [ $((dropped * 100)) -lt $((max_lost * total)) ]; fi'
  assert "$n videos: CPU ${CPU_PCT}% <= ${max_cpu}%" '[ "$CPU_PCT" -le "$max_cpu" ]'
}

# Small videos (S6 and S15), the hwdec switch of a big video, and a create/destroy leak check. The CPU limits are
# loose regression guards: CPU on a shared machine varies by about 10 % between runs (see architecture.md, Phase 1.3).
c13() {
  small_videos 6 50 0
  small_videos 15 90 1
  fresh
  "$OW" add "$FIX/h264_1080p.mp4" --id big --scale 0.25 --pos 50,50 >/dev/null
  shown big
  assert "big video switches to hardware decoding (got $(jo big .hwdec))" 'wait_until 10 "[ \"\$(jo big .hwdec)\" = nvdec ]"'
  local bb
  bb=$($PIX bbox "$(snap big $MAIN 1)" $(region "$(jo big .rect | jq -c .)" 1))
  assert "no black or missing frames after the switch (n=$(kv "$bb" n))" '[ "$(kv "$bb" n)" -ge 129000 ]'
  "$OW" remove big >/dev/null
  # Pss steps up by ~20 MB until about cycle 30 (allocator and driver warm-up) and is flat after it; a leak shows as
  # steady growth, extra threads or open descriptors.
  local p30 res30 pid
  pid=$(pgrep -x aniwayd)
  for i in $(seq 1 50); do
    "$OW" add "$FIX/h264_1080p.mp4" --id big --scale 0.25 >/dev/null
    sleep 1.2
    "$OW" remove big >/dev/null
    sleep 0.5
    [ $i = 30 ] && sleep 2 && p30=$(pss) && res30="$(ls /proc/$pid/task | wc -l)/$(ls /proc/$pid/fd | wc -l)"
  done
  sleep 2
  local res50="$(ls /proc/$pid/task | wc -l)/$(ls /proc/$pid/fd | wc -l)"
  assert "create/destroy cycles do not leak (Pss at cycle 30: $p30 MB, at 50: $(pss) MB; threads/fds $res30 -> $res50)" \
    '[ $(($(pss) - p30)) -le 15 ] && [ "$res30" = "$res50" ]'
}

c14() {
  fresh
  local scene=$XDG_CONFIG_HOME/aniway/scenes/default.toml
  "$OW" add "$FIX/anim.gif" --id s --pos 10,10 >/dev/null
  shown s
  local events
  events=$(python3 -I "$ROOT/tests/e2e/ipc.py" subscribe 1.5 '{"cmd":"set","id":"s","patch":{"x":77}}')
  assert "subscribe is acknowledged, then the current state arrives" 'head -1 <<<"$events" | jq -e ".ok" >/dev/null && sed -n 2p <<<"$events" | jq -e ".event == \"state\" and .data.overlays[0].id == \"s\"" >/dev/null'
  assert "event after set carries the new state" 'tail -1 <<<"$events" | jq -e ".data.overlays[0].x == 77" >/dev/null'
  local sum
  sum=$(md5sum <"$scene")
  python3 -I "$ROOT/tests/e2e/ipc.py" request '{"cmd":"set","id":"s","patch":{"x":120},"transient":true}' >/dev/null
  assert "transient set applies live" '[ "$(jo s .x)" = 120 ]'
  assert "transient set leaves the TOML untouched" '[ "$(md5sum <"$scene")" = "$sum" ] && grep -q "^x = 77" "$scene"'
  "$OW" set s --opacity 0.5 >/dev/null
  assert "normal set saves the whole state" 'grep -q "^x = 120" "$scene" && grep -q "^opacity = 0.5" "$scene"'
  python3 -I "$ROOT/tests/e2e/ipc.py" request '{"cmd":"set","id":"s","patch":{"x":5},"transient":true}' >/dev/null
  "$OW" reload >/dev/null
  assert "explicit reload discards transient changes" '[ "$(jo s .x)" = 120 ]'
  local cfg=$XDG_CONFIG_HOME/aniway/config.toml
  python3 -I "$ROOT/tests/e2e/ipc.py" request '{"cmd":"config_set","patch":{"hwdec":"vaapi","proxy_format":"compressed","hide_for_classes":["mpv"]}}' >/dev/null
  assert "config_set persists to config.toml" 'grep -q "^hwdec = \"vaapi\"" "$cfg" && grep -q "^proxy_format = \"compressed\"" "$cfg" && grep -q "^hide_for_classes = \[\"mpv\"\]" "$cfg"'
  assert "state carries the config (CLI get)" '[ "$("$OW" config get hwdec)" = vaapi ] && [ "$("$OW" config get hide_for_classes)" = "[\"mpv\"]" ]'
  "$OW" config set hwdec_min_height=900 optimize=false >/dev/null
  assert "CLI config set merges onto the rest" '[ "$("$OW" config get hwdec_min_height)" = 900 ] && [ "$("$OW" config get hwdec)" = vaapi ] && [ "$("$OW" config get optimize)" = false ]'
  assert "an invalid patch is refused and changes nothing" '! "$OW" config set proxy_cache_mb=0 2>/dev/null && ! "$OW" config set scene=x 2>/dev/null && [ "$("$OW" config get proxy_cache_mb)" = 4096 ]'
  for i in $(seq 2 15); do "$OW" add "$FIX/anim.gif" --id s$i --pos $((i * 10)),10 >/dev/null; done
  assert "a subscriber that stops reading is disconnected" '[ "$(python3 -I "$ROOT/tests/e2e/ipc.py" stall s)" = closed ]'
  assert "closed or dropped subscribers cause no warnings" '! grep -qE "WARN|ERROR" "$T/daemon.log" && pgrep -x aniwayd >/dev/null'
}

# Optimized copy of a big 60 fps alpha animation drawn small: it is built, played instead of the original (CPU drops),
# keeps the layout of the original, and a path change drops it.
c15() {
  fresh owtest
  rm -rf "$XDG_CACHE_HOME"
  local before after dir=$XDG_CACHE_HOME/aniway/proxies
  "$OW" add "$FIX/big_alpha.apng" --id big --output "$MAIN" --size 300x0 --pos 100,100 >/dev/null
  shown big
  assert "the copy is requested once the size settles" 'wait_until 8 "[ \"\$(jo big .proxy)\" != off ]"'
  cpu_rss "original while the copy builds" 6
  before=$CPU_PCT
  assert "the copy becomes ready" 'wait_until 90 "[ \"\$(jo big .proxy)\" = ready ]"'
  assert "the copy is uncompressed and in the private cache" 'ls "$dir"/*.mkv >/dev/null 2>&1 && ! ls "$dir"/*.part >/dev/null 2>&1'
  assert "the player switched to the copy" 'wait_until 8 "grep -q \"plays its optimized copy\" \"\$T/daemon.log\""'
  sleep 3
  cpu_rss "optimized copy" 10
  after=$CPU_PCT
  assert "CPU drops with the copy (${before}% -> ${after}%)" '[ $((after * 2)) -le "$before" ]'
  assert "the layout keeps the size of the original (rect $(jo big '[.rect.w, .rect.h] | join("x")'))" '[ "$(jo big .rect.w)" = 300 ] && [ "$(jo big .rect.h)" = 300 ]'
  assert "the surface survived the switch (dropped $(jo big .dropped))" '[ "$(jo big .shown)" = true ]'
  assert "the daemon survived with no warnings" '! grep -qE "WARN|ERROR" "$T/daemon.log" && pgrep -x aniwayd >/dev/null'
  daemon_stop
  daemon_start
  assert "after a restart the copy is played from the start" 'wait_until 10 "grep -q \"plays its optimized copy\" \"\$T/daemon.log\""'
  shown big
  assert "the original was never loaded: peak RSS $(awk '/VmHWM/ { printf "%.0f", $2 / 1024 }' /proc/$(pgrep -x aniwayd)/status) MB <= 400" '[ "$(awk "/VmHWM/ { printf \"%.0f\", \$2 / 1024 }" /proc/$(pgrep -x aniwayd)/status)" -le 400 ]'
  assert "the restarted overlay reports its copy as ready" 'wait_until 10 "[ \"\$(jo big .proxy)\" = ready ]"'
  printf 'raw_proxy_max_mb = 1\n' >"$XDG_CONFIG_HOME/aniway/config.toml"
  assert "over the per-file limit the copy is compressed instead" 'wait_until 90 "ls \"$dir\"/*.webm >/dev/null 2>&1 && [ \"\$(jo big .proxy)\" = ready ]"'
  assert "the uncompressed copy in use was not evicted before the switch" '[ "$(jo big .error)" = null ] && shown big'
  rm -f "$XDG_CONFIG_HOME/aniway/config.toml"
  daemon_stop
  rm -rf "$dir"
  mkdir -p "$T/nobin"
  PATH="$T/nobin" "$OW" daemon >"$T/daemon.log" 2>&1 &
  wait_until 10 '"$OW" list >/dev/null'
  assert "without ffmpeg the copy fails apart from playback" 'wait_until 15 "[ \"\$(jo big .proxy)\" = failed ]" && [ "$(jo big .error)" = null ] && [ -n "$(jo big .proxy_error // empty)" ]'
  assert "without ffmpeg the original keeps playing" 'shown big'
  daemon_stop
  daemon_start
  "$OW" set big --path "$FIX/anim.apng" >/dev/null
  assert "a path change drops the copy" 'wait_until 8 "[ \"\$(jo big .proxy)\" = off ]"'
  assert "the new media plays its original" 'shown big && [ "$(jo big .error)" = null ]'
  "$OW" add "$FIX/alpha.webm" --id web --output "$MAIN" --size 96x0 --pos 500,100 >/dev/null
  assert "a WebM with alpha gets an uncompressed copy, never an opaque one" 'wait_until 30 "[ \"\$(jo web .proxy)\" = ready ]" && ! ls "$dir"/*.mp4 >/dev/null 2>&1'
  daemon_stop
  daemon_start
  assert "after a restart the WebM copy is found before playing (its alpha is probed)" 'wait_until 10 "grep -q \"overlay .web. plays its optimized copy\" \"\$T/daemon.log\""'
}

cleanup() {
  daemon_stop 2>/dev/null
  pkill -x mpv 2>/dev/null
  nested_stop
  if [ "$CRIT_FAILED" -eq 0 ]; then rm -rf "$T"; else log "artifacts kept in $T"; fi
}

preflight
trap cleanup EXIT

declare -A DESC=(
  [1]="clean build, clippy and tests" [2]="single instance and clean SIGTERM" [3]="fixtures: size, transparency, sharpness"
  [4]="HEVC 4K 60 s loop with hwdec; CPU/RSS" [5]="click-through" [6]="live edits persist; hot reload; broken TOML"
  [7]="fullscreen per monitor and hide_for_classes" [8]="hide --for" [9]="output hotplug" [10]="fractional scale 1.25"
  [11]="anchors after a resolution change" [12]="rotated monitor" [13]="15 small videos, hwdec switch, leaks"
  [14]="subscribe, transient set" [15]="optimized copies"
)
selected=("$@")
[ ${#selected[@]} -eq 0 ] && selected=(1 2 3 4 5 6 7 8 9 10 11 12 13 14 15)

for n in "${selected[@]}"; do
  if [ "$n" != 1 ] && [ -z "${HPID:-}" ]; then
    nested_start || { log "cannot start the nested Hyprland (see $T/hypr.log)"; CRIT_FAILED=1; exit 2; }
  fi
  run_criterion "$n" "${DESC[$n]}" "c$n"
done

log
[ "$CRIT_FAILED" -eq 0 ] && log "ALL PASS" || log "$CRIT_FAILED criterion/criteria FAILED"
exit $((CRIT_FAILED > 0))
