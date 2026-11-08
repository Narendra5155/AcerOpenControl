# Installing AcerOpenControl

AcerOpenControl ships as two Debian packages:

| Package | Contains | Needs |
|---|---|---|
| `acer-open-control` | the `aocd` service, the `aocctl` CLI, model profiles | `acpi-call-dkms` |
| `acer-open-control-gtk` | the AcerOpenControl window | GTK 4, libadwaita, Python 3 |

Supported systems: Ubuntu 24.04 or newer on amd64 (tested on 26.04). Debian 13 should also
work. Other distributions can build from source.

## 1. From the APT repository (recommended)

```sh
curl -fsSL https://narendra5155.github.io/acer-open-control/key.gpg \
  | sudo tee /usr/share/keyrings/acer-open-control.gpg >/dev/null
echo "deb [signed-by=/usr/share/keyrings/acer-open-control.gpg] https://narendra5155.github.io/acer-open-control stable main" \
  | sudo tee /etc/apt/sources.list.d/acer-open-control.list
sudo apt update
sudo apt install acer-open-control-gtk      # or just acer-open-control for CLI only
```

The repository is signed. The first command saves its public key, and
`signed-by=` limits that key to this repository only. Future versions install
with `sudo apt upgrade`.

## 2. From a downloaded .deb

Download both files from the [releases page](https://github.com/narendra5155/acer-open-control/releases),
then install them together so apt can resolve dependencies:

```sh
sudo apt install ./acer-open-control_*_amd64.deb ./acer-open-control-gtk_*_all.deb
```

This way you don't get automatic updates.

## 3. From source

See [BUILDING.md](BUILDING.md). In short: `packaging/build-all.sh` produces both
`.deb` files in `dist/`.

## After installing

The service starts automatically. Check it:

```sh
aocctl status
aocctl model          # which model profile was picked
```

- If `aocctl status` prints **READ-ONLY**, no model profile matches your laptop.
  Nothing is written to the firmware; see [ADDING_A_MODEL.md](ADDING_A_MODEL.md).
- Changing settings needs an account in `sudo`, `admin`, `wheel` or
  `acer-open-control`. The membership is checked on every request, so it works
  immediately: no logout needed. For a non-admin user:
  `sudo usermod -aG acer-open-control USERNAME`.

## Secure Boot

`acpi-call-dkms` builds a kernel module. With Secure Boot enabled, Ubuntu signs
DKMS modules with a machine-owner key (MOK) that must be enrolled once:

1. During `apt install`, a text screen asks you to choose a password. Remember it.
2. Reboot. A blue **MOK management** screen appears. Choose *Enroll MOK* →
   *Continue* → *Yes*, enter the password, then *Reboot*.
3. Check: `sudo modprobe acpi_call && ls /proc/acpi/call`.

If you missed the screen: `sudo update-secureboot-policy --enroll-key`, then reboot.

## Uninstalling

```sh
sudo apt remove acer-open-control-gtk acer-open-control   # keeps /etc/acer-open-control
sudo apt purge acer-open-control                           # also deletes settings
```

Removing the package hands the fans back to the BIOS. To drop the repository too:

```sh
sudo rm /etc/apt/sources.list.d/acer-open-control.list /usr/share/keyrings/acer-open-control.gpg
```

## Migrating from the AcerFan prototype

If you used the earlier Python version (`install.sh`, service `acerfan`):

1. Install the packages as above. The package stops the old `acerfan` service,
   and `aocd` copies your settings from `/etc/acerfan/config.json` on first start.
2. Then remove the prototype's files: `sudo ./uninstall.sh` from the old checkout,
   or manually:
   `sudo rm -rf /opt/acerfan /usr/local/bin/acerfan /etc/systemd/system/acerfan.service /usr/lib/systemd/system-sleep/acerfan`.
