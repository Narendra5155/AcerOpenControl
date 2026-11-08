# Changelog

## 0.3.0

- **CoolerControl integration.** New package `acer-open-control-coolercontrol`:
  a CoolerControl device-service plugin that exposes the fans and aocd's CPU/GPU
  temperatures. See docs/COOLERCONTROL.md.
- **aocd fan lease API** (`claim_fans`, `set_fan_duty`, `release_fans`). Another
  program can borrow the fans per fan, while the emergency override, minimum
  floor and BIOS fallback stay active. The lease ends automatically when that
  program's connection closes.
- **Per-fan BIOS hand-back.**
- **New access level for lease users** (`lease_users`, default `cc-plugin-user`):
  they may borrow the fans and nothing else.
- **The GUI and `aocctl status`** show when another program controls the fans.
- **`packaging/bump-version.sh`** updates every version location at once.

## 0.2.2

First public release.

- Backend rewritten in Rust: `aocd` daemon, `aocctl` CLI, `aoc-core` library.
- Model profiles (`models/*.toml`): laptop support is data, not code. New
  `aocctl probe fans|features|keys` and `aocctl profiles generate` to create one.
- Read-only mode on laptops without a matching profile.
- Changing settings works for any administrator without re-login (per-request
  group check via `SO_PEERCRED`).
- GTK frontend split into its own package; hides features a model lacks; works
  with libadwaita older than 1.7.
- Debian packages and a signed APT repository on GitHub Pages.

### Planned
- Tray icon.
- Optional D-Bus interface with polkit.
- More model profiles from the community.

## 0.1.0 (prototype)

Python prototype for the Acer Aspire 7 A715-79G: fan curves, keyboard RGB,
performance profiles, charge limit, webcam switch, Fn hotkeys, GTK window.
