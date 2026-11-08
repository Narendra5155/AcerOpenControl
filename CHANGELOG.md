# Changelog

## 0.2.0 (unreleased)

First public release, renamed from the AcerFan prototype to **AcerOpenControl**.

- Backend rewritten in Rust: `aocd` daemon, `aocctl` CLI, `aoc-core` library.
- Model profiles (`models/*.toml`): laptop support is data, not code. New
  `aocctl probe fans|features|keys` and `aocctl profiles generate` to create one.
- Read-only mode on laptops without a matching profile.
- Changing settings works for any administrator without re-login (per-request
  group check via `SO_PEERCRED`).
- GTK frontend split into its own package; hides features a model lacks; works
  with libadwaita older than 1.7.
- Debian packages and a signed APT repository on GitHub Pages.
- Settings from `/etc/acerfan/config.json` are migrated automatically.

### Planned
- Tray icon.
- Optional D-Bus interface with polkit.
- More model profiles from the community.

## 0.1.0 (prototype)

Python prototype for the Acer Aspire 7 A715-79G: fan curves, keyboard RGB,
performance profiles, charge limit, webcam switch, Fn hotkeys, GTK window.
