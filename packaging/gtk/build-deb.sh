#!/bin/sh
# Build the acer-open-control-gtk .deb (architecture: all).
# Usage: packaging/gtk/build-deb.sh [OUT_DIR]   (default: dist/)
set -eu
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
OUT="${1:-$ROOT/dist}"
. "$ROOT/packaging/meta.env"

VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' "$ROOT/Cargo.toml" | head -1)"
PYVER="$(sed -n 's/^__version__ = "\(.*\)"/\1/p' "$ROOT/frontend/aoc_gtk/__init__.py")"
PYAPP="$(sed -n 's/^APP_ID = "\(.*\)"/\1/p' "$ROOT/frontend/aoc_gtk/__init__.py")"
[ "$VERSION" = "$PYVER" ] || { echo "version mismatch: Cargo.toml $VERSION vs aoc_gtk $PYVER" >&2; exit 1; }
CLVER="$(sed -n '1s/^[^ ]* (\([^)]*\)).*/\1/p' "$ROOT/packaging/debian/changelog")"
DEBVER="$CLVER"   # e.g. 0.2.0-1
[ "$VERSION" = "${CLVER%-*}" ] || { echo "version mismatch: Cargo.toml $VERSION vs packaging/debian/changelog $CLVER" >&2; exit 1; }
[ "$APP_ID" = "$PYAPP" ] || { echo "APP_ID mismatch: meta.env $APP_ID vs aoc_gtk $PYAPP" >&2; exit 1; }

PKG=acer-open-control-gtk
STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT
subst() { sed -e "s/@DEVELOPER_ID@/$DEVELOPER_ID/g" -e "s/@APP_ID@/$APP_ID/g" -e "s/@OWNER@/$OWNER/g" -e "s/@REPO@/$REPO/g" -e "s/@VERSION@/$VERSION/g" "$1"; }

install -d "$STAGE/DEBIAN" "$STAGE/usr/bin" "$STAGE/usr/lib/python3/dist-packages/aoc_gtk" \
    "$STAGE/usr/share/applications" "$STAGE/usr/share/icons/hicolor/scalable/apps" \
    "$STAGE/usr/share/metainfo" "$STAGE/usr/share/doc/$PKG"
install -m 644 "$ROOT"/frontend/aoc_gtk/*.py "$STAGE/usr/lib/python3/dist-packages/aoc_gtk/"
cat > "$STAGE/usr/bin/aoc-gtk" <<'PY'
#!/usr/bin/python3
from aoc_gtk.app import main

raise SystemExit(main())
PY
chmod 755 "$STAGE/usr/bin/aoc-gtk"
subst "$ROOT/packaging/gtk/app.desktop.in" > "$STAGE/usr/share/applications/$APP_ID.desktop"
subst "$ROOT/packaging/gtk/app.metainfo.xml.in" > "$STAGE/usr/share/metainfo/$APP_ID.metainfo.xml"
install -m 644 "$ROOT/packaging/gtk/icon.svg" "$STAGE/usr/share/icons/hicolor/scalable/apps/$APP_ID.svg"

cat > "$STAGE/usr/share/doc/$PKG/copyright" <<COPY
Format: https://www.debian.org/doc/packaging-manuals/copyright-format/1.0/
Upstream-Name: $REPO
Source: https://github.com/$OWNER/$REPO

Files: *
Copyright: 2026 Narendra Hindwar
License: GPL-3.0-or-later
 On Debian systems, the full text of the GNU General Public License
 version 3 can be found in the file '/usr/share/common-licenses/GPL-3'.
COPY
sed "s/^acer-open-control /$PKG /" "$ROOT/packaging/debian/changelog" | gzip -9n > "$STAGE/usr/share/doc/$PKG/changelog.Debian.gz"
install -d "$STAGE/usr/share/man/man1"
gzip -9n < "$ROOT/packaging/man/aoc-gtk.1" > "$STAGE/usr/share/man/man1/aoc-gtk.1.gz"

SIZE="$(du -sk "$STAGE/usr" | cut -f1)"
cat > "$STAGE/DEBIAN/control" <<CTL
Package: $PKG
Version: $DEBVER
Architecture: all
Maintainer: $MAINTAINER
Installed-Size: $SIZE
Depends: acer-open-control (>= $VERSION), python3 (>= 3.10), python3-gi, gir1.2-gtk-4.0, gir1.2-adw-1
Section: utils
Priority: optional
Homepage: https://github.com/$OWNER/$REPO
Description: GTK interface for AcerOpenControl
 Window for the AcerOpenControl daemon: live temperatures and fan speeds,
 fan modes and a draggable fan curve, keyboard backlight colour and
 brightness, performance profile, battery charge limit and webcam switch.
 Unofficial; not affiliated with Acer.
CTL

find "$STAGE" -type d -exec chmod 755 {} +
find "$STAGE/usr" -type f -exec chmod 644 {} +
chmod 755 "$STAGE/usr/bin/aoc-gtk"
mkdir -p "$OUT"
dpkg-deb --root-owner-group -Zxz --build "$STAGE" "$OUT/${PKG}_${DEBVER}_all.deb" >/dev/null
echo "$OUT/${PKG}_${DEBVER}_all.deb"
