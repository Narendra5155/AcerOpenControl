# Contributing

Thanks for helping! The most valuable contributions are **model profiles** for
laptops we don't support yet; see [docs/ADDING_A_MODEL.md](docs/ADDING_A_MODEL.md).

## Reporting a bug

Include:
- `aocctl status`, `aocctl model` and `aocctl profiles list`
- `journalctl -u aocd -b --no-pager | tail -50`
- what you did, what you expected, what happened

Don't attach `acpidump` output: it contains your Windows product key (MSDM table).

## Code changes

1. Read [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).
2. Keep model-specific values out of the code; they belong in profiles.
3. New firmware commands need a reference to the DSDT method they come from, and
   proof from real hardware, before they ship. When in doubt, leave a feature
   out: see the "deliberately not exposed" list in [docs/HARDWARE.md](docs/HARDWARE.md).
4. Every fail-safe path needs a test. `aoc_core::fake::FakeFirmware` makes that
   possible without hardware.
5. Before opening a pull request:

   ```sh
   cargo fmt --all && cargo clippy --workspace --all-targets --all-features -- -D warnings
   cargo test --workspace --all-features
   (cd frontend && python3 -m unittest discover -s tests -t .)
   ```

By contributing, you agree that your contribution is licensed under GPL-3.0-or-later.
