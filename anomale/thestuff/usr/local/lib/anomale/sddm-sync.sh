#!/bin/bash
# Copy the current user's pywal SDDM colors + wallpaper into the system theme.
# Intended to run as root via pkexec (polkit) or during install via sudo.
set -euo pipefail

THEME_DIR="/usr/share/sddm/themes/anomalous"
THEME_USER_CONF="$THEME_DIR/theme.conf.user"
THEME_BG="$THEME_DIR/background.jpg"

resolve_user() {
    local uid name home
    if [[ -n "${PKEXEC_UID:-}" ]]; then
        uid=$PKEXEC_UID
    elif [[ -n "${SUDO_UID:-}" ]]; then
        uid=$SUDO_UID
    elif [[ -n "${SUDO_USER:-}" ]]; then
        name=$SUDO_USER
        uid=$(id -u "$name")
    else
        echo "anomale-sddm-sync: must be invoked via pkexec or sudo" >&2
        exit 1
    fi

    if ! name=$(id -nu "$uid" 2>/dev/null); then
        echo "anomale-sddm-sync: cannot resolve user for uid $uid" >&2
        exit 1
    fi
    if ! home=$(getent passwd "$uid" | cut -d: -f6) || [[ -z "$home" ]]; then
        echo "anomale-sddm-sync: cannot resolve home for uid $uid" >&2
        exit 1
    fi

    printf '%s\t%s\t%s\n' "$uid" "$name" "$home"
}

if [[ "$(id -u)" -ne 0 ]]; then
    echo "anomale-sddm-sync: must run as root" >&2
    exit 1
fi

IFS=$'\t' read -r _user_uid _user_name user_home < <(resolve_user)

wal_conf="$user_home/.cache/wal/sddm.conf"
wal_path_file="$user_home/.cache/wal/wal"

if [[ ! -f "$wal_conf" ]]; then
    echo "anomale-sddm-sync: missing $wal_conf" >&2
    exit 1
fi
if [[ ! -f "$wal_path_file" ]]; then
    echo "anomale-sddm-sync: missing $wal_path_file" >&2
    exit 1
fi
if [[ ! -d "$THEME_DIR" ]]; then
    echo "anomale-sddm-sync: theme not installed at $THEME_DIR" >&2
    exit 1
fi

wallpaper=$(tr -d '\n' < "$wal_path_file")
if [[ -z "$wallpaper" ]]; then
    echo "anomale-sddm-sync: empty wallpaper path in $wal_path_file" >&2
    exit 1
fi
if [[ "$wallpaper" != /* ]]; then
    echo "anomale-sddm-sync: wallpaper path must be absolute" >&2
    exit 1
fi
wallpaper=$(readlink -f "$wallpaper")
if [[ ! -f "$wallpaper" ]]; then
    echo "anomale-sddm-sync: wallpaper is not a regular file: $wallpaper" >&2
    exit 1
fi
# Ensure the invoking user can read the source (reject paths only root could open).
if ! runuser -u "$_user_name" -- test -r "$wallpaper"; then
    echo "anomale-sddm-sync: wallpaper not readable by ${_user_name}: $wallpaper" >&2
    exit 1
fi

# theme.conf.user overrides packaged theme.conf; keep background relative to theme dir.
install -m 644 "$wal_conf" "$THEME_USER_CONF"
# Force background key to the copied asset name regardless of template drift.
if grep -qE '^[[:space:]]*background=' "$THEME_USER_CONF"; then
    sed -i -E 's|^[[:space:]]*background=.*|background=background.jpg|' "$THEME_USER_CONF"
else
    if grep -qE '^[[:space:]]*\[General\]' "$THEME_USER_CONF"; then
        sed -i -E '/^[[:space:]]*\[General\]/a background=background.jpg' "$THEME_USER_CONF"
    else
        printf '[General]\nbackground=background.jpg\n' | cat - "$THEME_USER_CONF" >"${THEME_USER_CONF}.tmp"
        mv "${THEME_USER_CONF}.tmp" "$THEME_USER_CONF"
        chmod 644 "$THEME_USER_CONF"
    fi
fi

install -m 644 "$wallpaper" "$THEME_BG"
chown root:root "$THEME_USER_CONF" "$THEME_BG"
