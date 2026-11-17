#!/usr/bin/env bash
# Setup sekali-jalan yang butuh root (dijalankan via sudo):
#   sudo bash /home/mikay/Projects/trofeo-lcd/scripts/root-setup.sh
#
# 1. Pasang dependency build (libusb/hidapi headers) kalau belum ada.
# 2. Pasang udev rule supaya LCD Thermalright bisa diakses user biasa
#    tanpa root (bulk LY 0416:5408/5409 dan varian HID 0416:5302).
# 3. Reload udev dan re-apply rule ke device yang sudah terpasang.
set -euo pipefail

if [[ $EUID -ne 0 ]]; then
    echo "Jalankan dengan sudo: sudo bash $0" >&2
    exit 1
fi

REPO_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

echo "== [1/3] Dependency build (apt) =="
if command -v apt-get >/dev/null 2>&1; then
    apt-get update -qq || true
    apt-get install -y --no-install-recommends \
        build-essential pkg-config libusb-1.0-0-dev libudev-dev libhidapi-dev \
        libpulse-dev || true
else
    echo "apt-get tidak ada — asumsikan dependency sudah terpasang."
fi

echo "== [2/3] Udev rule =="
install -m 0644 "$REPO_DIR/99-trofeo-lcd.rules" /etc/udev/rules.d/99-trofeo-lcd.rules

echo "== [3/3] Reload udev & re-trigger =="
udevadm control --reload-rules
udevadm trigger --subsystem-match=usb --action=change || true
# Re-apply ke node hidraw yang sudah ada (MODE/GROUP tidak re-evaluate
# otomatis oleh trigger usb; jalan langsung juga pada hidraw).
udevadm trigger --subsystem-match=hidraw --action=change || true

echo "Selesai. Cek akses:"
ls -la /dev/hidraw* 2>/dev/null || true
