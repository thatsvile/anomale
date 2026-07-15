#!/bin/bash
# Keep Anomale boot splash wired into mkinitcpio (GRUB and systemd-boot both use this).
set -euo pipefail

SPLASH="/usr/local/share/anomale/splash.bmp"
PRESET_DIR="/etc/mkinitcpio.d"

if [[ ! -f "$SPLASH" ]]; then
    echo "anomale-splash: missing $SPLASH" >&2
    exit 1
fi

shopt -s nullglob
presets=("$PRESET_DIR"/*.preset)
if ((${#presets[@]} == 0)); then
    echo "anomale-splash: no presets in $PRESET_DIR" >&2
    exit 0
fi

changed=0
for preset in "${presets[@]}"; do
    if grep -q -- '--splash ' "$preset"; then
        if grep -q -- "--splash $SPLASH" "$preset"; then
            continue
        fi
        sed -i -E "s|--splash[[:space:]]+[^\"[:space:]]+|--splash $SPLASH|g" "$preset"
        changed=1
        echo "anomale-splash: updated --splash in $preset"
    fi
done

if ((changed == 1)) || [[ "${1:-}" == "--force-rebuild" ]]; then
    echo "anomale-splash: rebuilding initramfs/UKI images..."
    mkinitcpio -P
else
    echo "anomale-splash: presets already point at $SPLASH"
fi
