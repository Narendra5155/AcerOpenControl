#!/bin/sh
# Remove AcerFan and hand the fans back to the BIOS. Run: sudo ./uninstall.sh
set -u
[ "$(id -u)" -eq 0 ] || { echo "run with sudo" >&2; exit 1; }

systemctl disable --now acerfan.service 2>/dev/null
PYTHONPATH=/opt/acerfan /usr/bin/python3 -m acerfan restore-auto 2>/dev/null
rm -f /etc/systemd/system/acerfan.service /usr/lib/systemd/system-sleep/acerfan \
      /etc/modules-load.d/acerfan.conf /usr/local/bin/acerfan \
      /usr/share/applications/io.github.acerfan.AcerFan.desktop \
      /usr/share/icons/hicolor/scalable/apps/acerfan.svg
rm -rf /opt/acerfan
systemctl daemon-reload
echo "Removed. Settings kept in /etc/acerfan (delete it manually if you like)."
echo "The 'acerfan' group was left in place; remove it with: sudo groupdel acerfan"
