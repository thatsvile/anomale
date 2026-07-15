 #!/bin/bash


export LIBVA_DRIVER_NAME=nvidia

export XDG_SESSION_TYPE=wayland

export __GLX_VENDOR_LIBRARY_NAME=nvidia

export ELECTRON_OZONE_PLATFORM_HINT=auto

export NVD_BACKEND=direct

export GBM_BACKEND=nvidia-drm

export XDG_CURRENT_DESKTOP=wlroots


eval $(gnome-keyring-daemon --start --components=secrets)

export SQLITE_TMPDIR=/tmp


/usr/lib/polkit-gnome/polkit-gnome-authentication-agent-1 &

gsettings set org.gnome.desktop.interface color-scheme 'prefer-dark'

anomale &

systemctl --user start mangowm-session.target
dbus-update-activation-environment --systemd --all

# When LibreWolf first opens, push pywal colors without a manual "Fetch" click.
(
    for _ in $(seq 1 24); do
        sleep 10
        if pgrep -x librewolf >/dev/null 2>&1; then
            pywalfox update >/dev/null 2>&1 && break
        fi
    done
) &
