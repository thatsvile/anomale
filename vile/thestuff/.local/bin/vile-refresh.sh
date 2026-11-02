#!/bin/bash
pkill -x nautilus
pkill -f xdg-desktop-portal-gnome
pkill -f xdg-desktop-portal-gtk
pkill -f xdg-desktop-portal
pkill -f polkit-gnome-authentication-agent-1 && /usr/lib/polkit-gnome/polkit-gnome-authentication-agent-1 &
sleep 0.5
gsettings set org.gnome.desktop.interface gtk-theme 'Adwaita'
sleep 0.2
gsettings set org.gnome.desktop.interface gtk-theme 'adw-gtk3-dark'
sleep 0.3
systemctl --user start xdg-desktop-portal xdg-desktop-portal-gtk xdg-desktop-portal-gnome
bash ~/.local/bin/folder-col.sh

# Sync vile/wal colors + wallpaper into the root-owned SDDM theme via polkit helper.
if [[ -x /usr/local/lib/vile/sddm-sync.sh ]]; then
    pkexec /usr/local/lib/vile/sddm-sync.sh
fi
