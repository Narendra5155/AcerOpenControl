# Using AcerOpenControl

Everything below can be done from the window (**AcerOpenControl** in the app grid)
or with `aocctl`. Both talk to the `aocd` service, so changes made in one show up
in the other within a second.

## Fans

| Mode | What happens |
|---|---|
| **Auto** | The laptop's BIOS controls the fans, as if AcerOpenControl wasn't installed. |
| **Silent** | Slow until the machine gets warm (0% below 50 °C, 100% at 92 °C). |
| **Balanced** | Default. 0% below 45 °C, 100% at 90 °C. |
| **Performance** | Cools early: 20% from 40 °C, 100% at 86 °C. |
| **Max** | All fans at 100%. |
| **Custom** | Your curve. |

The temperature that drives the curve is the **hotter of the CPU and GPU**. The GPU
is only read while it's awake; a sleeping GPU is never woken up to read its
temperature.

**How the fans react:**

- **Speeding up** happens immediately.
- **Slowing down** waits until the temperature has dropped 4 °C, then goes down
  at most 5% per second-and-a-half. This stops the fan from pulsing.
- **Minimum speed:** the fans never go below 20% while the hotter chip is at 60 °C or more.

**Editing the curve:** on the *Fans* tab, drag any point. The dashed line shows
the current temperature. Click **Apply as Custom** to save and switch to it. From
the CLI:

```sh
aocctl curve                          # show the custom curve
aocctl curve 50:0 65:30 80:70 90:100  # set it (TEMP:DUTY pairs) and switch to custom
aocctl mode balanced
```

Rules for curve points: temperatures increase from point to point (20–105 °C),
duty never decreases (0–100%), and a curve needs at least 2 points.

**Fail-safes (always on):**

- **Emergency:** CPU at 92 °C or more, or GPU at 85 °C or more, forces 100%
  until it cools down. A model profile can tune these limits.
- **Sensor failure:** 3 failed CPU readings in a row hand the fans back to the BIOS.
- **Firmware errors:** 3 firmware errors in a row hand the fans back to the BIOS.
- **Service stops:** the fans go back to the BIOS. That includes crashes and
  `kill -9`, because systemd runs `aocctl restore-auto` after the service exits.

## Keyboard backlight

On the *Keyboard* tab: switch the light on and off, set brightness, and pick a colour
with the swatches or the colour picker (RGB keyboards only).

```sh
aocctl kbd --color orange --brightness 180
aocctl kbd --color 00ffaa
aocctl kbd --brightness off
```

Named colours: white, red, green, blue, orange, yellow, purple, cyan, pink.

## Fn hotkeys

On the A715-79G these keys do nothing on stock Linux. AcerOpenControl makes them work:

| Key | Action |
|---|---|
| Fn + F1 | Touchpad on/off (GNOME shows its usual popup) |
| Fn + F10 | Webcam on/off |
| Fn + numpad − / + | Keyboard light dimmer / brighter (9 steps) |
| Fn + / | Keyboard light off / back on at the previous level |
| Fn + * | Cycle keyboard colour |

After resuming from sleep the keys take about 2 seconds to start working again,
because the firmware forgets the registration while asleep.

## Performance profile (System tab)

These are the firmware's own power modes. On the A715-79G:

| Profile | CPU sustained / boost | GPU |
|---|---|---|
| Quiet | 15 W / 30 W | 60 W |
| Power saver | 15 W / 30 W | 60 W |
| Balanced | 45 W / 115 W | 60 W |
| Performance | 90 W / 115 W | 70 W |

```sh
aocctl profile performance     # keys are listed by `aocctl model`
aocctl profile bios            # stop re-applying a profile at boot
```

AcerOpenControl only re-applies a profile at boot and after resume if you chose one.

## Battery charge limit

Keeping a battery at 100% while plugged in wears it faster. With a limit, charging
stops at the chosen level and resumes once the battery drops 5% below it.

```sh
aocctl charge 80        # stop at 80 %, resume below 75 %
aocctl charge 60 --start 50
aocctl charge off       # charge to 100 %
```

## Webcam

```sh
aocctl webcam off       # the camera disappears from the system (not just muted)
aocctl webcam on
```

## Status and model information

```sh
aocctl status           # human-readable
aocctl status --json    # everything the daemon reports
aocctl model            # matched profile, capabilities, profile names
aocctl profiles list    # all installed profiles; * marks the one matching this laptop
```

## Configuration file

Settings live in `/etc/acer-open-control/config.json`, written by the daemon.
You don't normally edit it by hand; if you do, run `sudo systemctl restart aocd`
afterwards. An invalid file is ignored and defaults are used.

| Key | Default | Meaning |
|---|---|---|
| `mode` | `"balanced"` | fan mode |
| `custom_curve` | balanced preset | `[[temp, duty], ...]` |
| `poll_interval` | `1.5` | seconds between control steps (0.5–10) |
| `refresh_interval` | `30` | resend the fan duty at least this often (s) |
| `hysteresis` | `4.0` | °C the temperature must fall before slowing down |
| `ramp_down` | `5.0` | max % decrease per step |
| `force_profile` | `null` | use this profile id even if the laptop doesn't match it (dangerous) |
| `features.hotkeys` | `true` | handle Fn hotkeys |
| `features.gnome_kbd_slider` | `true` | expose the keyboard light as a system LED |
| `features.keymap` | `{}` | extra or overriding hotkey mappings, e.g. `{"0x9F": "kbd-toggle"}` |
| `features.saved` | | what you chose; reapplied at boot and resume |
