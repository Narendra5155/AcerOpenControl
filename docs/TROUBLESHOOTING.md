# Troubleshooting

## Collect information first

```sh
aocctl status
aocctl model
aocctl profiles list
journalctl -u aocd -b --no-pager | tail -50
```

Include that output when you open an issue.

## "aocd is not running"

```sh
sudo systemctl status aocd
journalctl -u aocd -b --no-pager | tail -30
```

The cause is usually one of the problems below.

## `/proc/acpi/call is missing`

The `acpi_call` kernel module isn't loaded.

```sh
sudo modprobe acpi_call
```

- **"Key was rejected by service":** Secure Boot is on and the DKMS key isn't
  enrolled. See [INSTALL.md → Secure Boot](INSTALL.md#secure-boot).
- **"Module not found":** reinstall the module for your current kernel with
  `sudo apt install --reinstall acpi-call-dkms`. Make sure
  `linux-headers-$(uname -r)` is installed.

## The status says READ-ONLY

No installed model profile matches this laptop, so AcerOpenControl only shows
temperatures. `aocctl profiles list` shows the detected product name and whether
the Clevo WMI interface exists.

- **Interface present:** you can create a profile ([ADDING_A_MODEL.md](ADDING_A_MODEL.md)).
- **Not present:** this laptop uses a different firmware interface and isn't supported.

## "permission denied: changing settings needs an administrator account"

Your account isn't in `sudo`, `admin`, `wheel` or `acer-open-control`. An administrator can fix that:

```sh
sudo usermod -aG acer-open-control USERNAME
```

It takes effect immediately; there's no need to log out.

## Fans don't change speed

- Check `aocctl status`: under **control** it should say a percentage, not BIOS.
  It says BIOS in Auto mode or after a fault; look for `FAULT` lines.
- Another fan tool may be fighting it. Run `systemctl status nbfc_service`; if
  it's active, disable it with `sudo systemctl disable --now nbfc_service`.
- Duty readback works but the fans stay quiet: the model profile's fan commands
  may be wrong. Run `sudo systemctl stop aocd && sudo aocctl probe fans`.

## Fn keys don't work

- After resume they need about 2 seconds.
- Check the log for `Fn hotkeys enabled` and for `hotkey 0x..` lines when you
  press a key: `journalctl -u aocd -f`.
- A key that shows `unmapped` needs an entry in the profile's `[hotkeys.map]`,
  or in `features.keymap` in the config ([USAGE.md](USAGE.md#configuration-file)).
- To see raw codes:

  ```sh
  sudo systemctl stop aocd && sudo aocctl probe keys --raw 30; sudo systemctl start aocd
  ```

## GNOME's keyboard-brightness slider is missing

GNOME looks for a keyboard light once, at login. AcerOpenControl creates one when
its service starts, so the slider appears only if the service was already running
when you logged in, which is normal after a reboot. The Fn keys and the
AcerOpenControl window work either way.

## The charger plugged/unplugged and the GPU power didn't change

With hotkeys registered, the firmware leaves this step to the driver, and the
daemon runs it (`charger connected: ran \_SB.AC.ADJP` in the log). If it's
missing, check that `/sys/class/power_supply/*/type` contains a `Mains` entry.

## Getting back to stock

```sh
aocctl mode auto               # BIOS fan control, daemon keeps running
sudo systemctl stop aocd       # daemon stopped, fans back to BIOS
sudo apt remove acer-open-control
```
