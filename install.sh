#!/bin/sh
# Install AcerFan (daemon + CLI). Run: sudo ./install.sh
set -eu

if [ "$(id -u)" -ne 0 ]; then
    echo "run with sudo: sudo ./install.sh" >&2
    exit 1
fi

SRC="$(cd "$(dirname "$0")" && pwd)"
PRODUCT="$(cat /sys/class/dmi/id/product_name 2>/dev/null || echo unknown)"
case "$PRODUCT" in
    *A715-79G*) ;;
    *)
        echo "This was built and tested for the Acer Aspire A715-79G; this machine is: $PRODUCT" >&2
        [ "${ACERFAN_FORCE:-0}" = 1 ] || { echo "Set ACERFAN_FORCE=1 to install anyway." >&2; exit 1; }
        ;;
esac

if ! modinfo acpi_call >/dev/null 2>&1; then
    echo "acpi_call kernel module missing: sudo apt install acpi-call-dkms" >&2
    exit 1
fi

echo "-> installing files to /opt/acerfan"
rm -rf /opt/acerfan
mkdir -p /opt/acerfan
cp -r "$SRC/acerfan" /opt/acerfan/
find /opt/acerfan -name __pycache__ -prune -exec rm -rf {} +
chown -R root:root /opt/acerfan
chmod -R go-w /opt/acerfan

cat > /usr/local/bin/acerfan <<'EOF'
#!/bin/sh
PYTHONPATH=/opt/acerfan exec /usr/bin/python3 -m acerfan "$@"
EOF
chmod 755 /usr/local/bin/acerfan

echo acpi_call > /etc/modules-load.d/acerfan.conf
install -m 644 "$SRC/packaging/acerfan.service" /etc/systemd/system/acerfan.service
install -m 755 "$SRC/packaging/acerfan-sleep" /usr/lib/systemd/system-sleep/acerfan

echo "-> app launcher and icon"
install -Dm 644 "$SRC/packaging/acerfan.desktop" /usr/share/applications/io.github.acerfan.AcerFan.desktop
install -Dm 644 "$SRC/packaging/acerfan.svg" /usr/share/icons/hicolor/scalable/apps/acerfan.svg
gtk-update-icon-cache -q /usr/share/icons/hicolor 2>/dev/null || true
update-desktop-database -q /usr/share/applications 2>/dev/null || true

echo "-> group 'acerfan' (members can change fan settings without sudo)"
getent group acerfan >/dev/null || groupadd --system acerfan
if [ -n "${SUDO_USER:-}" ] && [ "$SUDO_USER" != root ]; then
    usermod -aG acerfan "$SUDO_USER"
    echo "   added $SUDO_USER (log out and back in for it to take effect)"
fi

if systemctl is-enabled nbfc_service >/dev/null 2>&1 || systemctl is-active nbfc_service >/dev/null 2>&1; then
    echo "-> disabling nbfc_service (two fan controllers would fight)"
    systemctl disable --now nbfc_service || true
fi

systemctl daemon-reload
systemctl enable acerfan.service
systemctl restart acerfan.service
sleep 2
systemctl --no-pager --lines=5 status acerfan.service || true
echo
echo "Done. Open \"AcerFan\" from the app grid, or run: acerfan gui"
echo "CLI: acerfan status   |   acerfan mode silent|balanced|performance|max|auto"
