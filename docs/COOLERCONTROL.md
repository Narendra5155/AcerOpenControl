# Using AcerOpenControl with CoolerControl

[CoolerControl](https://gitlab.com/coolercontrol/coolercontrol) is a popular Linux
app for fan curves across all the cooling devices in a computer. It can't reach
these laptops' fans on its own, because they are driven through firmware calls
rather than the kernel's hwmon interface. The `acer-open-control-coolercontrol`
package adds a CoolerControl **device plugin** that gives CoolerControl access to them.

## Install

You need CoolerControl 5.0 or newer and AcerOpenControl 0.3.0 or newer.

```sh
sudo apt install acer-open-control-coolercontrol
```

The package installs the plugin and restarts `coolercontrold` if it's running.
In CoolerControl a new device, **AcerOpenControl**, appears with:

| Channel | What it is |
|---|---|
| CPU Fan, GPU Fan | the laptop's fans: speed (rpm) and duty (%); controllable |
| CPU (aocd), GPU (aocd) | temperatures as AcerOpenControl reads them |

The **GPU (aocd)** temperature is useful on laptops with switchable graphics.
AcerOpenControl doesn't wake a sleeping GPU just to read its temperature; it
simply reports nothing while the GPU sleeps. Use it as the GPU temperature
source in your profiles if CoolerControl's own GPU reading keeps the GPU awake.

## How control is shared

CoolerControl **borrows** the fans from AcerOpenControl; it never talks to the
firmware itself.

- **CoolerControl takes over** as soon as you apply a profile or fixed speed to
  one of the fans. AcerOpenControl's own fan mode pauses. The AcerOpenControl
  window then shows "Fans controlled by: CoolerControl" and locks the mode
  buttons. `aocctl status` shows the same.
- **Per fan:** a fan CoolerControl hasn't set (or has reset to *Default*) is
  driven by the BIOS.
- **Giving the fans back:** set both fans back to the *Default* profile in
  CoolerControl, or stop CoolerControl. AcerOpenControl's own mode (Balanced,
  Silent, …) resumes within a second. The same happens if the plugin or
  CoolerControl crashes, because the borrowing is tied to the plugin's
  connection to `aocd`.

**AcerOpenControl's fail-safes stay active while CoolerControl drives the fans:**

- CPU 92 °C / GPU 85 °C forces both fans to 100%.
- Duties are raised to at least 20% once the hotter chip is at 60 °C or more,
  so a "fans off" profile can't cook the laptop under load.
- Losing the CPU temperature, or repeated firmware errors, hand the fans to the BIOS.

These limits come from the model profile's `[safety]` section.

## Permissions

CoolerControl runs plugins as the restricted `cc-plugin-user` account. `aocd`
lets that account do exactly one thing: borrow the fans (`claim_fans`,
`set_fan_duty`, `release_fans`). It can't change modes, the keyboard light, the
charge limit or anything else. The list of such accounts is `lease_users` in
`/etc/acer-open-control/config.json`; it defaults to `["cc-plugin-user"]`.

## Troubleshooting

```sh
journalctl -u coolercontrold -b | grep -i aoc        # plugin start-up as seen by CoolerControl
aocctl status                                        # shows "control: CoolerControl: fan1 …" while borrowed
journalctl -u aocd -b | grep -i lease                # "fans lent to CoolerControl" / "lease … ended"
```

| Symptom | Fix |
|---|---|
| No AcerOpenControl device in CoolerControl | `sudo systemctl restart coolercontrold`; check that `/var/lib/coolercontrol/plugins/aoc-coolercontrol/manifest.toml` exists |
| "permission denied … lease_users" | the plugin runs as an account not in `lease_users`; add it to the config and `sudo systemctl restart aocd` |
| Fans faster than the CoolerControl profile | a fail-safe is active: check `aocctl status` for EMERGENCY, or the 20% floor above 60 °C |
| Device shows only temperatures | the laptop has no model profile (aocd is read-only); see [ADDING_A_MODEL.md](ADDING_A_MODEL.md) |

## Removing

```sh
sudo apt remove acer-open-control-coolercontrol
```

CoolerControl is restarted without the plugin and AcerOpenControl takes the fans back.

## For developers

- **Plugin code:** `backend/aoc-coolercontrol`. It's a tonic gRPC server
  implementing CoolerControl's `DeviceService`; the definitions are vendored in
  `backend/aoc-coolercontrol/proto`.
- **End-to-end test without hardware or CoolerControl:**

  ```sh
  cargo run -p aocd --features dev-fake -- --fake --socket $XDG_RUNTIME_DIR/aocd.sock --config /tmp/c.json &
  cargo run -p aoc-coolercontrol -- --socket $XDG_RUNTIME_DIR/cc.sock --aocd $XDG_RUNTIME_DIR/aocd.sock &
  cargo run -p aoc-coolercontrol --example e2e -- $XDG_RUNTIME_DIR/cc.sock
  ```

  `examples/e2e.rs` plays CoolerControl's part. It sends `Health`,
  `ListDevices`, `Status`, `EnableManualFanControl`, `FixedDuty`,
  `ResetChannel` and `Shutdown` over real gRPC.
