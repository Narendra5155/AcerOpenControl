# The firmware interface

This page records what is known about the firmware interface AcerOpenControl
drives, worked out from the A715-79G's DSDT (BIOS 1.07.01RAC5) and confirmed on
the machine. It is the background for [model profiles](ADDING_A_MODEL.md).

Raw ACPI dumps are deliberately **not** in this repository: the MSDM table in an
`acpidump` contains the laptop's embedded Windows product key.

## Why Linux had no fan control

The A715-79G is built on a **Clevo NP50RN** board. Linux's `acer-wmi` driver looks
for Acer's gaming WMI GUID (`7A4DDFE7-…`), which this firmware doesn't have, so
`acer-wmi` doesn't load. There is no `platform_profile` and no hwmon PWM. The
firmware does carry Clevo's WMI interface, the one TUXEDO's Clevo driver uses:

| Object | Value |
|---|---|
| WMI method GUID | `ABBC0F6D-8EA1-11D1-00A0-C90629100000` → `\_SB.WMI.WMBB` |
| WMI event GUIDs | `ABBC0F6B-…` (notify `0xD0`), `ABBC0F6C-…` (`0xD1`) |
| Clevo hotkey device | `\_SB.DCHU`, `_HID "CLV0001"` |

## Calling the firmware

`WMBB(Arg0, Arg1 = command, Arg2 = 4-byte buffer)` takes the EC mutex `PATM`,
then dispatches to `GCMD` (getters), `SCMD` (setters) or `CC20`. `Arg2` must be a
**buffer**: the firmware evaluates `SizeOf(Arg2)` and converts it to an integer
little-endian. Setters return their own command id on success and `Ones` if the
mutex couldn't be taken.

AcerOpenControl calls it through the `acpi_call` kernel module:

```
echo '\_SB.WMI.WMBB 0x0 0x68 bff800000' > /proc/acpi/call ; cat /proc/acpi/call
```

Going through `WMBB` means every EC access takes the firmware's own lock. Tools
that write EC registers directly (`ec_sys`) skip it and can race the firmware.

## Command table (A715-79G)

| Cmd | Kind | Argument → effect | DSDT EC command |
|---|---|---|---|
| `0x68` | set | byte *n* → duty of fan *n*+1 (0–255) | `C1 0n` |
| `0x69` | set | bitmask → hand those fans back to the EC | `C1 FF n` |
| `0x63` `0x64` `0x6E` | get | fan info words (duty and temperature bytes) | `C0 xx` |
| `0x6B` | set | `INDX = size_code<<16 \| offset` | — |
| `0x73` | get | read 1/2/4 bytes of EC RAM at `0xFE0B0100 + offset` | — |
| `0x67` | set | `0xF0…\|B<<16\|R<<8\|G`: colour · `0xF4…\|level`: brightness | `CA 03`, `CA 06` |
| `0x79` | set | `0x19<<24\|n`: performance profile *n* (via table `APPM`, notifies DPTF and NVIDIA `NPCF`) | `D8 01` |
| `0x79` | set | `0x06<<24\|stop<<8\|start`, then `0x05<<24\|enable`: battery charge limit | `CB` |
| `0x77` | get | `stop<<16 \| start<<8 \| enabled` | — |
| `0x22` | set | 0 = webcam off, 1 = on | `B8 A1` |
| `0x46` | set | register as hotkey driver (`WMI.HKDR = 1`); returns PSF3 | — |
| `0x01` | get | code of the last hotkey (`WMI.EVNT`) | — |
| `0x52` `0x60` `0x7A` | get | feature words PSF1, PSF4, PSF2 (PSF4 bit 2 = performance profiles) | — |

## EC RAM (memory-mapped at `0xFE0B0100`)

| Offset | Field | Meaning |
|---|---|---|
| `0xCE` / `0xCF` | `DUT1` / `DUT2` | current duty of fan 1 / 2 (0–255) |
| `0xD0` / `0xD2` | `RPM1` / `RPM2` | tachometer, **big-endian**, rpm = 2156220 / raw |
| `0xD4`, `0xE0` | `RPM4`, `RPM3` | unused on this model |

Measured: duty 255 → 6360 / 5780 rpm; duty 76 → 2500 / 2370 rpm; BIOS idle duty
71 → about 2350 rpm. The EC keeps a manual duty indefinitely (it doesn't time
out), and `0x69` hands control back cleanly.

## Performance profiles (measured)

| Index | Name used | CPU PL1 / PL2 | GPU limit |
|---|---|---|---|
| 0 | Quiet | 15 W / 30 W | 60 W |
| 1 | Power saver | 15 W / 30 W | 60 W |
| 3 | Balanced | 45 W / 115 W | 60 W |
| 2 | Performance | 90 W / 115 W | 70 W |

Before any profile is set, the BIOS boots with PL1 = 200 W. The current index is
kept in the ACPI name `\_SB.PC00.LPCB.EC.CPCM`.

## Hotkeys

Special Fn keys go through EC query `_Q50`, then `NEVT(code)`. `NEVT` sends
`Notify(WMI, 0xD0)` and stores the code in `WMI.EVNT`, **but only if a driver has
registered** (`WMI.HKDR` or `DCHU.HKDR` set). Otherwise the key press is dropped.
That's why these keys do nothing on stock Linux. The kernel's WMI core forwards
the notify to userspace as an `acpi_event` generic-netlink message.

| Key | Code |
|---|---|
| Fn+F1 (touchpad) | `0xFD` (`0xFC`) |
| Fn+F10 (camera) | `0xF7` |
| Fn+numpad − / + | `0x81` / `0x82` |
| Fn+/ | `0x83` |
| Fn+* | `0x9F` |
| performance key (if present) | `0x59` |

**Side effects of registering:**

- **Sleep resets it:** `_PTS` clears `HKDR`, so the driver re-registers after resume.
- **Charger changes:** with `HKDR` set, `\_SB.AC._PSR` stops calling `AC.ADJP`
  on charger changes (it adjusts GPU power between AC and battery, because
  `PSF1 & 0x30 = 0x20`). The daemon calls `\_SB.AC.ADJP 0` itself whenever
  mains power changes.
- **Profile key:** `_Q5D` (performance key) sends event `0x59` instead of cycling
  the profile itself.

## Deliberately not exposed

| Command | Why |
|---|---|
| `0x4F` | switches dGPU/hybrid graphics mode via `AC.ADJP` and can leave the machine without a display |
| `0x48` | forces the dGPU on |
| `0x49`, `0x5A` | fake power-button, sleep-button and lid events |
| `0x5B`, `0x13`, `0x79/0x13` | SMI and flash parameters |
| `0x79/0x07` | thermal limit derived from MXTJ |
| `0x75` | raw EC RAM write |
| `0x20`, `0x2A` | wireless and touchpad switches: no effect on this model |
