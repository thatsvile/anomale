#!/bin/bash
# Usage: vile-refresh.sh [dark|light]
# Mode comes from arg, else VILE_THEME, else colors.json vile.theme, else dark.

pkill -x nautilus
pkill -f xdg-desktop-portal-gnome
pkill -f xdg-desktop-portal-gtk
pkill -f xdg-desktop-portal
pkill -f polkit-gnome-authentication-agent-1 && /usr/lib/polkit-gnome/polkit-gnome-authentication-agent-1 &
sleep 0.5

resolve_theme() {
    case "${1:-}" in
        dark|light) echo "$1"; return ;;
    esac
    case "${VILE_THEME:-}" in
        dark|light) echo "$VILE_THEME"; return ;;
    esac
    local cache="${XDG_CACHE_HOME:-$HOME/.cache}"
    local json=""
    for candidate in "$cache/wal/colors.json" "$cache/vile/wal/colors.json"; do
        if [[ -f "$candidate" ]]; then
            json="$candidate"
            break
        fi
    done
    if [[ -n "$json" ]]; then
        local theme
        theme=$(python3 -c 'import json,sys; d=json.load(open(sys.argv[1])); print((d.get("vile") or {}).get("theme") or "")' "$json" 2>/dev/null || true)
        case "$theme" in
            dark|light) echo "$theme"; return ;;
        esac
    fi
    echo dark
}

THEME="$(resolve_theme "${1:-}")"
if [[ "$THEME" == "light" ]]; then
    COLOR_SCHEME=prefer-light
    GTK_THEME=adw-gtk3
else
    COLOR_SCHEME=prefer-dark
    GTK_THEME=adw-gtk3-dark
fi

gsettings set org.gnome.desktop.interface color-scheme "$COLOR_SCHEME"
gsettings set org.gnome.desktop.interface gtk-theme 'Adwaita'
sleep 0.2
gsettings set org.gnome.desktop.interface gtk-theme "$GTK_THEME"
sleep 0.3
systemctl --user start xdg-desktop-portal xdg-desktop-portal-gtk xdg-desktop-portal-gnome
bash ~/.local/bin/folder-col.sh

# Sync vile/wal colors + wallpaper into the root-owned SDDM theme via polkit helper.
if [[ -x /usr/local/lib/vile/sddm-sync.sh ]]; then
    pkexec /usr/local/lib/vile/sddm-sync.sh
fi
