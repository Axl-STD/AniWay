#!/usr/bin/env bash
# Re-extracts the strings of the editor and of the daemon tray menu into po/aniway.pot and merges them into every po/*.po.
# New strings come out empty: translate them, then check with `msgfmt --statistics -c po/es.po`.
set -euo pipefail
cd "$(dirname "$0")/../crates/aniway-editor"
xgettext -L Rust --from-code=UTF-8 -k -ktr -kngettext:1,2 -kspin:1 -kswitch:1 -kcombo:1 -kexpander:1 \
  -kslider_row:1 -kgroup:1 -kitem:1 -khide_for:1 --package-name=aniway -o po/aniway.pot src/*.rs \
  ../aniwayd/src/tray.rs
for po in po/*.po; do
  msgmerge --update --backup=none --no-fuzzy-matching --quiet "$po" po/aniway.pot
  msgattrib --no-obsolete -o "$po" "$po"
  msgfmt --statistics -o /dev/null "$po"
done
