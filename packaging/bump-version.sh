#!/bin/sh
# Set the project version everywhere it lives, then refresh Cargo.lock.
#   packaging/bump-version.sh 0.2.3 ["Changelog line"]
# Updates: Cargo.toml, Cargo.lock, frontend/aoc_gtk/__init__.py,
# packaging/debian/changelog (new top entry), man page headers.
# CHANGELOG.md is left for you to write.
set -eu
NEW="${1:?usage: $0 X.Y.Z [\"changelog line\"]}"
MSG="${2:-New upstream release.}"
echo "$NEW" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+$' || { echo "version must look like 1.2.3" >&2; exit 1; }
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
. packaging/meta.env

OLD="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
sed -i "0,/^version = \".*\"/s//version = \"$NEW\"/" Cargo.toml
sed -i "s/^__version__ = \".*\"/__version__ = \"$NEW\"/" frontend/aoc_gtk/__init__.py
sed -i -E "s/^(\.TH [A-Z-]+ [0-9] \"[^\"]*\" \"[a-z-]+ )[0-9.]+\"/\1$NEW\"/" packaging/man/*.[18]

if ! head -1 packaging/debian/changelog | grep -q "($NEW-1)"; then
    tmp="$(mktemp)"
    {
        echo "acer-open-control ($NEW-1) stable; urgency=medium"
        echo
        echo "  * $MSG" | fold -s -w 76 | sed '2,$s/^/    /'
        echo
        echo " -- $MAINTAINER  $(date -R)"
        echo
        cat packaging/debian/changelog
    } > "$tmp"
    mv "$tmp" packaging/debian/changelog
fi

cargo update --workspace --offline >/dev/null 2>&1 || cargo update --workspace >/dev/null
echo "version $OLD -> $NEW"
echo "Now: update CHANGELOG.md, commit, then: git tag -a v$NEW -m v$NEW && git push origin main v$NEW"
