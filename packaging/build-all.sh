#!/bin/sh
# Build both .deb packages into OUT_DIR (default: dist/).
set -eu
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
OUT="${1:-$ROOT/dist}"
case "$OUT" in /*) ;; *) OUT="$ROOT/$OUT" ;; esac
cd "$ROOT"
mkdir -p "$OUT"
cargo build --release --workspace --locked
cargo deb -p aocd --no-build --output "$OUT"
packaging/gtk/build-deb.sh "$OUT"
ls -l "$OUT"/*.deb
