#!/bin/bash

export XDG_SESSION_TYPE=wayland
export ELECTRON_OZONE_PLATFORM_HINT=auto
export NVD_BACKEND=direct
export XDG_CURRENT_DESKTOP=niri

eval $(gnome-keyring-daemon --start --components=secrets)
export SQLITE_TMPDIR=/tmp

/usr/lib/polkit-gnome/polkit-gnome-authentication-agent-1 &

gsettings set org.gnome.desktop.interface color-scheme 'prefer-dark'

# Let niri finish bringing up Wayland/portals before layer-shell registration.
sleep 1
vile --refresh

dbus-update-activation-environment --systemd --all
