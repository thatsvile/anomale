#!/bin/bash

export LIBVA_DRIVER_NAME=nvidia
export XDG_SESSION_TYPE=wayland
export __GLX_VENDOR_LIBRARY_NAME=nvidia
export ELECTRON_OZONE_PLATFORM_HINT=auto
export NVD_BACKEND=direct
export GBM_BACKEND=nvidia-drm
export XDG_CURRENT_DESKTOP=niri

eval $(gnome-keyring-daemon --start --components=secrets)
export SQLITE_TMPDIR=/tmp

/usr/lib/polkit-gnome/polkit-gnome-authentication-agent-1 &

# Restore last colorlab Theme base (prefer-dark fallback).
export CACHE="${XDG_CACHE_HOME:-$HOME/.cache}"
THEME=$(python3 -c '
import json, os, sys
cache = os.environ.get("CACHE", "")
for path in (f"{cache}/wal/colors.json", f"{cache}/vile/wal/colors.json"):
    try:
        theme = (json.load(open(path)).get("vile") or {}).get("theme")
        if theme in ("dark", "light"):
            print(theme)
            sys.exit(0)
    except Exception:
        pass
print("dark")
' 2>/dev/null || echo dark)
if [[ "$THEME" == "light" ]]; then
    gsettings set org.gnome.desktop.interface color-scheme 'prefer-light'
    gsettings set org.gnome.desktop.interface gtk-theme 'adw-gtk3'
else
    gsettings set org.gnome.desktop.interface color-scheme 'prefer-dark'
    gsettings set org.gnome.desktop.interface gtk-theme 'adw-gtk3-dark'
fi

# Let niri finish bringing up Wayland/portals before layer-shell registration.
sleep 1
vile --refresh

dbus-update-activation-environment --systemd --all
