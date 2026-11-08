# Building and testing

## Toolchain

```sh
# Rust 1.75 or newer, with rustfmt and clippy (CI pins 1.93.1, same as Ubuntu 26.04)
sudo apt install cargo rustc rustfmt rust-clippy        # or: rustup (https://rustup.rs)
cargo install cargo-deb --locked                        # builds the backend .deb

# Frontend and packaging
sudo apt install python3-gi gir1.2-gtk-4.0 gir1.2-adw-1 lintian apt-utils
```

## Tests

```sh
cargo test --workspace --all-features
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo fmt --all --check
(cd frontend && python3 -m unittest discover -s tests -t . -v)
```

The Rust tests run against `aoc_core::fake::FakeFirmware`, an emulation of the
A715-79G's `WMBB` firmware, so they need neither root nor the hardware.

## Running from a checkout

### Without hardware: the fake daemon

```sh
cargo run -p aocd --features dev-fake -- --fake \
    --socket "$XDG_RUNTIME_DIR/aocd.sock" --config /tmp/aoc-config.json &
cargo run -p aocctl -- --socket "$XDG_RUNTIME_DIR/aocd.sock" status
AOC_SOCKET="$XDG_RUNTIME_DIR/aocd.sock" PYTHONPATH=frontend python3 -m aoc_gtk
```

`--fake` runs the full daemon against the firmware emulator, with fixed
temperatures (CPU 72 °C, GPU 61 °C). It's what the screenshots in the README use.
For an end-to-end test of the CoolerControl plugin over real gRPC, see
[COOLERCONTROL.md → For developers](COOLERCONTROL.md#for-developers).

### On the real laptop

```sh
cargo build --release
sudo systemctl stop aocd                      # if the package is installed
sudo RUST_LOG=debug target/release/aocd --models models/
```

`--models DIR` loads profiles from `DIR` only. Stopping the daemon with Ctrl+C
hands the fans back to the BIOS.

## Packages

```sh
packaging/build-all.sh          # → dist/acer-open-control_<v>_amd64.deb, dist/acer-open-control-gtk_<v>_all.deb
lintian dist/*.deb
sudo apt install ./dist/*.deb
```

- **Backend package:** built by `cargo deb` from `[package.metadata.deb]` in
  `backend/aocd/Cargo.toml`. The maintainer scripts are in `packaging/debian/`.
- **GUI package:** built by `packaging/gtk/build-deb.sh`, from
  `packaging/gtk/*.in` and `packaging/meta.env`.
- **CoolerControl plugin package:** built by `cargo deb -p aoc-coolercontrol`.
  Its manifest and scripts are in `packaging/coolercontrol/`.

## Version numbers

The version lives in several places; `packaging/bump-version.sh X.Y.Z` updates them
all, and the build refuses to continue if they disagree:

- `version` in the root `Cargo.toml`
- `__version__` in `frontend/aoc_gtk/__init__.py`
- the top entry of `packaging/debian/changelog`
