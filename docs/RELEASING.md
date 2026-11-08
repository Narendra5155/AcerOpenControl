# Releasing and publishing the APT repository

Releases are built by GitHub Actions when a `vX.Y.Z` tag is pushed:

- **Release page:** both `.deb` files are attached to a GitHub Release.
- **APT repository:** the files are added to a signed APT repository served by
  GitHub Pages from the `gh-pages` branch.

## One-time setup

### 1. The GitHub repository

The project lives at `github.com/Narendra5155/AcerOpenControl`. `OWNER` and `REPO`
in `packaging/meta.env` must match it exactly, because GitHub Pages paths are
case-sensitive. For a fork, run `packaging/set-owner.sh NEW_OWNER` and commit the
result; it updates the URLs, the app ID and `packaging/meta.env`.

### 2. Create the repository signing key

Use a dedicated key, separate from your personal one:

```sh
gpg --batch --quick-gen-key "AcerOpenControl APT <hindwarnarendra@gmail.com>" ed25519 sign 3y
KEYID=$(gpg --list-secret-keys --with-colons "AcerOpenControl APT" | awk -F: '/^fpr/ {print $10; exit}')
gpg --armor --export-secret-keys "$KEYID" > apt-signing-key.asc   # you'll set a passphrase
```

Add two repository secrets under **Settings → Secrets and variables → Actions**:

| Secret | Value |
|---|---|
| `APT_GPG_PRIVATE_KEY` | contents of `apt-signing-key.asc` |
| `APT_GPG_PASSPHRASE` | its passphrase |

Then store `apt-signing-key.asc` somewhere safe and offline, and delete the
local copy. If the key is lost, every user has to re-import a new one.

### 3. Enable GitHub Pages

The first release creates the `gh-pages` branch. After it has run, set
**Settings → Pages → Source: Deploy from a branch → `gh-pages` / root**. The
repository is then served at `https://narendra5155.github.io/AcerOpenControl/`.

## Each release

1. Make sure `main` is green in CI.
2. Bump the version in `Cargo.toml` (`[workspace.package] version`) and
   `frontend/aoc_gtk/__init__.py`. Run `cargo build` to refresh `Cargo.lock`.
3. Add entries to `CHANGELOG.md`, and to `packaging/debian/changelog` in Debian
   format with the new version at the top.
4. Commit, tag and push:

   ```sh
   git commit -am "Release v0.2.1"
   git tag -a v0.2.1 -m "v0.2.1"
   git push origin main v0.2.1
   ```

5. Watch **Actions → Release**. The workflow refuses to run if the tag doesn't
   match the `Cargo.toml` version. It runs the tests, builds and lints both
   packages, creates the GitHub Release, and commits the updated repository to
   `gh-pages`.
6. Check the result from a clean system:

   ```sh
   docker run --rm -it ubuntu:24.04 bash -c '
     apt-get update && apt-get install -y curl ca-certificates &&
     curl -fsSL https://narendra5155.github.io/AcerOpenControl/key.gpg -o /usr/share/keyrings/acer-open-control.gpg &&
     echo "deb [signed-by=/usr/share/keyrings/acer-open-control.gpg] https://narendra5155.github.io/AcerOpenControl stable main" > /etc/apt/sources.list.d/aoc.list &&
     apt-get update && apt-get install -y --no-install-recommends acer-open-control-gtk && aocctl --version'
   ```

   Inside a container the service can't start; this only checks that the
   repository, its signature and the dependencies resolve.

## Publishing by hand

`packaging/apt/publish-repo.sh DEB_DIR REPO_DIR KEYID` does what CI does. It
needs `apt-utils` and the signing key in your keyring:

```sh
git clone --branch gh-pages https://github.com/Narendra5155/AcerOpenControl site
packaging/build-all.sh dist
packaging/apt/publish-repo.sh dist site "$KEYID"
cd site && git add -A && git commit -m "Publish vX.Y.Z" && git push
```

Old versions stay in `pool/main`, so users can still downgrade with `apt install pkg=version`.

## Rotating or revoking the signing key

1. Create a new key and replace both secrets.
2. Run the release workflow (or publish by hand). That re-signs the repository
   and publishes the new `key.gpg`.
3. Tell users, in the release notes, to re-run the `curl … key.gpg` line from the
   README. apt reports `NO_PUBKEY` until they do.
