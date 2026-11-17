#!/usr/bin/env bash
# Install Linux build dependencies and USB rules. Optional CPU power access
# is granted to the invoking user via the trofeo-rapl group.
set -euo pipefail

if (( $# > 1 )) || [[ ${1:-} != "" && ${1:-} != --rapl ]]; then
    echo "Usage: sudo bash $0 [--rapl]" >&2
    exit 2
fi
if (( EUID != 0 )); then
    echo "Run with sudo: sudo bash $0 [--rapl]" >&2
    exit 1
fi

repo_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
rule=/etc/udev/rules.d/99-trofeo-lcd.rules

if command -v apt-get >/dev/null; then
    apt-get update -qq
    apt-get install -y --no-install-recommends build-essential pkg-config \
        libusb-1.0-0-dev libudev-dev libpulse-dev
else
    echo "Install the libudev and PulseAudio development packages with your package manager."
fi

# Undo the old installer's grant on live nodes as well as replacing its rule.
if [[ -f "$rule" ]] && grep -Fq 'chmod 0444 /sys/class/powercap/' "$rule"; then
    for counter in /sys/class/powercap/intel-rapl*/energy_uj; do
        [[ -e "$counter" ]] || continue
        chmod 0400 "$counter"
    done
fi
install -m 0644 "$repo_dir/99-trofeo-lcd.rules" "$rule"

if [[ ${1:-} == --rapl ]]; then
    getent group trofeo-rapl >/dev/null || groupadd --system trofeo-rapl
    if [[ -n ${SUDO_USER:-} && $SUDO_USER != root ]]; then
        usermod -aG trofeo-rapl "$SUDO_USER"
        echo "Log out and back in for the new group membership."
    fi
    install -m 0644 "$repo_dir/scripts/99-trofeo-rapl.rules" /etc/udev/rules.d/
fi

udevadm control --reload-rules
udevadm trigger --subsystem-match=usb --attr-match=idVendor=0416 --action=change
udevadm trigger --subsystem-match=hidraw --action=change
if [[ -f /etc/udev/rules.d/99-trofeo-rapl.rules ]]; then
    udevadm trigger --subsystem-match=powercap --action=change
fi
udevadm settle
