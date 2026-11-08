#!/bin/sh
# Add .deb files to a flat-pool APT repository and (re)generate signed indexes.
#
#   packaging/apt/publish-repo.sh DEB_DIR REPO_DIR [GPG_KEY_ID]
#
# REPO_DIR is the GitHub Pages checkout (gh-pages branch). Existing packages in
# REPO_DIR/pool are kept, so older versions stay installable. The key must be
# in the current GnuPG keyring; GPG_KEY_ID defaults to $APT_GPG_KEY_ID.
# Needs: apt-utils (apt-ftparchive), gnupg.
set -eu

DEBS="${1:?usage: $0 DEB_DIR REPO_DIR [GPG_KEY_ID]}"
SITE="${2:?usage: $0 DEB_DIR REPO_DIR [GPG_KEY_ID]}"
KEY="${3:-${APT_GPG_KEY_ID:-}}"
[ -n "$KEY" ] || { echo "no signing key: pass GPG_KEY_ID or set APT_GPG_KEY_ID" >&2; exit 1; }
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
. "$ROOT/packaging/meta.env"
SUITE=stable
DIST="dists/$SUITE"

mkdir -p "$SITE/pool/main" "$SITE/$DIST/main/binary-amd64"
SITE="$(cd "$SITE" && pwd)"
found=0
for deb in "$DEBS"/*.deb; do
    [ -e "$deb" ] || continue
    cp -f "$deb" "$SITE/pool/main/"
    echo "added $(basename "$deb")"
    found=1
done
[ "$found" = 1 ] || { echo "no .deb files in $DEBS" >&2; exit 1; }

cd "$SITE"
# Architecture: all packages are listed in the amd64 index too (what apt expects
# when the Release file does not advertise a separate binary-all index).
apt-ftparchive --arch amd64 packages pool > "$DIST/main/binary-amd64/Packages"
gzip -9nkf "$DIST/main/binary-amd64/Packages"

apt-ftparchive \
    -o APT::FTPArchive::Release::Origin="$REPO" \
    -o APT::FTPArchive::Release::Label="AcerOpenControl" \
    -o APT::FTPArchive::Release::Suite="$SUITE" \
    -o APT::FTPArchive::Release::Codename="$SUITE" \
    -o APT::FTPArchive::Release::Architectures="amd64" \
    -o APT::FTPArchive::Release::Components="main" \
    -o APT::FTPArchive::Release::Description="AcerOpenControl packages (unofficial; not affiliated with Acer)" \
    release "$DIST" > "$DIST/Release.tmp"
mv "$DIST/Release.tmp" "$DIST/Release"

rm -f "$DIST/InRelease" "$DIST/Release.gpg"
gpg --batch --yes --default-key "$KEY" --clearsign -o "$DIST/InRelease" "$DIST/Release"
gpg --batch --yes --default-key "$KEY" -abs -o "$DIST/Release.gpg" "$DIST/Release"
gpg --batch --yes --export "$KEY" > key.gpg
gpg --batch --yes --armor --export "$KEY" > key.asc

cat > index.html <<HTML
<!doctype html>
<meta charset="utf-8">
<title>AcerOpenControl APT repository</title>
<style>body{font:16px/1.5 system-ui,sans-serif;max-width:46rem;margin:2rem auto;padding:0 1rem}pre{background:#f4f4f4;padding:1rem;overflow-x:auto}</style>
<h1>AcerOpenControl APT repository</h1>
<p>Unofficial fan, keyboard and firmware control for Clevo-based Acer laptops.
Not affiliated with or endorsed by Acer Inc.</p>
<pre>curl -fsSL $PAGES_URL/key.gpg | sudo tee /usr/share/keyrings/acer-open-control.gpg &gt;/dev/null
echo "deb [signed-by=/usr/share/keyrings/acer-open-control.gpg] $PAGES_URL $SUITE main" \\
  | sudo tee /etc/apt/sources.list.d/acer-open-control.list
sudo apt update
sudo apt install acer-open-control-gtk</pre>
<p>Documentation and supported models: <a href="https://github.com/$OWNER/$REPO">github.com/$OWNER/$REPO</a></p>
HTML
touch .nojekyll
echo "repository updated in $SITE"
