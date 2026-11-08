# Adding a laptop model

AcerOpenControl's support for a laptop is a **model profile**: a TOML file that
says which firmware commands do what on that model. Adding a model takes about
15 minutes with the probe tools and needs no programming.

## Does my laptop qualify?

Install the `acer-open-control` package, then run:

```sh
aocctl profiles list
```

Look for `Clevo WMI interface: present`. It means the firmware has the WMI method
GUID `ABBC0F6D-8EA1-11D1-00A0-C90629100000` (it also appears in
`/sys/bus/wmi/devices/`).

- **Present:** continue below. Many Acer Aspire 7 / Swift X / some Nitro models
  are built on Clevo boards.
- **Not present:** the laptop uses another interface (often `acer-wmi`, which the
  Linux kernel already supports for many Nitro/Predator models). AcerOpenControl
  can't help.

> **Safety.** The probes write to your firmware, using only commands proven
> safe on Clevo firmware. They briefly run the fans at 100% and 30%, change
> the keyboard light, cycle performance profiles, and switch the webcam off and
> on. Every probe restores the previous state. Do this on AC power, with nothing
> important running.

## 1. Probe

Stop the service first so it doesn't fight the probes, and work in an empty directory:

```sh
sudo systemctl stop aocd
mkdir ~/aoc-probe && cd ~/aoc-probe
sudo aocctl probe fans        # ~45 s; fans go loud then quiet; answer one question
sudo aocctl probe features    # interactive: watch the keyboard and answer y/n
sudo aocctl probe keys        # press each key it asks for (wait 10 s to skip)
```

| Probe | Learns | Saves |
|---|---|---|
| `probe fans` | whether manual fan duty works; how RPM is encoded | `aoc-probe-fans.json` |
| `probe features` | keyboard colour channel order; power limits of each performance profile; which value turns the webcam off; whether the charge limit is readable | `aoc-probe-features.json` |
| `probe keys` | the firmware code of each special Fn key | `aoc-probe-keys.json` |

The probes use the generic Clevo command set (the same as the A715-79G). If a
probe reports that something doesn't work, the matching section is left out of
the profile; that feature is then simply hidden.

## 2. Generate a draft profile

```sh
aocctl profiles generate . > my-laptop.toml
aocctl profiles check my-laptop.toml
```

The draft starts with `# NOTE:` lines for anything that needs a human look.
Open it and check the following:

- **`[model]`:** a readable `name`; `status = "experimental"` until others have tested it.
- **`[[performance.profile]]`:** give each index a `key` and `label` that match
  its measured power limits. Lowest power is usually "quiet" or "power-saver",
  highest is "performance".
- **`[hotkeys.map]`:** remove any key you didn't actually test.

## 3. Test it

```sh
sudo cp my-laptop.toml /etc/acer-open-control/models.d/
sudo systemctl start aocd
aocctl model                 # should show your profile
aocctl mode max              # fans loud?
aocctl mode auto
```

Profiles in `/etc/acer-open-control/models.d/` override shipped ones with the same `id`.

Check each feature from the window, plus the Fn keys, and do a suspend/resume
cycle. If anything misbehaves: `aocctl mode auto`, `sudo systemctl stop aocd`,
then fix the profile or remove the section.

## 4. Contribute it

Open a pull request that adds your file to `models/` and a row to the README's
**Supported laptops** table, with:

- the output of `aocctl profiles list` and `cat /sys/class/dmi/id/bios_version`
- the three `aoc-probe-*.json` files (they contain no personal data: only DMI
  names, temperatures and firmware values)
- what you tested

Never attach `acpidump` output: its MSDM table contains your Windows product key.

## Profile reference

Numbers can be written in hex (`0x68`) or decimal. Unknown keys are rejected,
which catches typos. The file is validated when the daemon starts; errors name
the file and the key.

### `[model]` (required)
| Key | Meaning |
|---|---|
| `id` | unique, lowercase letters, digits and `-` |
| `name` | shown in the app |
| `platform` | e.g. `clevo-np50rn` (informational) |
| `status` | `verified` · `community` · `experimental` |
| `notes` | free text |

### `[match]` (required)
A profile applies only when **all** given conditions hold.

| Key | Meaning |
|---|---|
| `dmi_product_name` | list; `/sys/class/dmi/id/product_name` must equal one of them |
| `dmi_board_name` | optional list, compared with `/sys/class/dmi/id/board_name` |
| `bios_version_prefix` | optional list of prefixes of `/sys/class/dmi/id/bios_version` |
| `wmi_guid` | the WMI method GUID that must exist (Clevo: `ABBC0F6D-8EA1-11D1-00A0-C90629100000`) |

### `[firmware]` (required)
| Key | Default | Meaning |
|---|---|---|
| `wmbb_path` | | ACPI path of the WMI method, e.g. `'\_SB.WMI.WMBB'`: use single quotes |
| `set_index_cmd` | `0x6B` | sets the EC RAM address for the next read |
| `read_ec_cmd` | `0x73` | reads EC RAM |

### `[fans]` (optional: no fan control without it)
| Key | Meaning |
|---|---|
| `count`, `labels` | number of fans (1–4) and their names |
| `set_duty_cmd` | command taking byte *n* = duty of fan *n* (0–255) |
| `set_auto_cmd`, `auto_mask` | hands fans back to the EC; mask bit *n* = fan *n* |
| `duty_offsets` | EC RAM offset of each fan's current duty byte |
| `rpm_offsets` | EC RAM offset of each fan's 2-byte tachometer value |
| `rpm_endian`, `rpm_dividend` | rpm = `rpm_dividend / raw`, raw read `big` or `little` endian |

### `[keyboard]` (optional)
| Key | Meaning |
|---|---|
| `kind` | `rgb-single-zone` or `white` |
| `cmd` | command id |
| `brightness_prefix` | argument = prefix \| level |
| `max_brightness` | highest level |
| `colour_prefix`, `channel_shift` | RGB only: argument = prefix \| R<<red \| G<<green \| B<<blue; the shifts are 0, 8 and 16 in some order |

### `[performance]` (optional)
`cmd`, `selector` (argument = selector<<24 | index), `current_path` (optional ACPI
name holding the current index), and one `[[performance.profile]]` per mode with
`index`, `key`, `label`, `description`.

### `[battery]` (optional)
`cmd`, `thresholds_selector` (argument = sel<<24 | stop<<8 | start),
`enable_selector` (argument = sel<<24 | 0/1), `read_cmd` (returns
stop<<16 | start<<8 | enabled).

### `[webcam]` (optional)
`cmd` and `off_value` (0 or 1; the other value turns it on).

### `[hotkeys]` (optional)
| Key | Meaning |
|---|---|
| `enable_cmd` | registers as the hotkey driver (needed after every boot and resume) |
| `read_cmd` | returns the code of the key just pressed |
| `notify`, `event_guid_prefix` | how the key event arrives over ACPI netlink |
| `ac_adjust_method` | optional; ACPI method the daemon calls on charger plug/unplug (see HARDWARE.md) |
| `[hotkeys.map]` | `"0xCODE" = "action"`; actions: `kbd-up`, `kbd-down`, `kbd-toggle`, `kbd-colour-cycle`, `touchpad-toggle`, `touchpad-on`, `touchpad-off`, `webcam-toggle`, `profile-cycle` |

### `[safety]` (optional)
`cpu_emergency` (default 92), `gpu_emergency` (85), `floor_pct` (20), `floor_temp` (60).
