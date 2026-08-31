# Anomale

[![Watch the example video](https://i.postimg.cc/28XHGczx/image-2.png)](https://www.youtube.com/watch?v=_Iyf3RlilNw)

Personal Arch Linux dots and a small Wayland shell, built around
[niri](https://github.com/YaLTeR/niri) and pywal.

This is software I maintain for my own machines. You can use it if you want.
It is not a general-purpose desktop, and it is not written with support in mind.

## Overview

Anomale is two pieces that ship together:

1. **Dotfiles** — niri session config, terminal/shell setup (`foot` / `fish`),
   GTK theming, SDDM theme, and pywal templates that keep colors consistent.
2. **Anomale Shell** — a thin GTK4 layer-shell interface on top of niri:
   status bar, app launcher, power menu, notifications, system tray, and a
   wallpaper picker that regenerates the pywal theme.

It does not try to replace a full desktop environment. No applet pile, no
heavily customized GUI apps. The point is to stay out of the way and keep you
in the terminal as much as possible. If that sounds annoying, this setup is
not for you — that is intentional.

Sources for the shell live under `anomale/thestuff/shell/`. Day to day you are
not meant to build that by hand; the install script builds it and installs the
dots.

## Stack

| Piece | Role |
| --- | --- |
| Arch Linux | Base system (official repos + a few upstream builds; no AUR) |
| niri | Window manager / compositor |
| Anomale | Bar, menus, notifications, tray, wallpaper → pywal |
| pywal16 | Color scheme from wallpaper (terminal, GTK, niri, SDDM, browser) |
| SDDM + Anomalous | Display manager and login theme |
| foot + fish | Default terminal and shell |

## Requirements

- Fresh Arch install (CachyOS / EndeavourOS may work; recent versions untested)
- Working network and a usable `pacman` mirrorlist
- No existing DE or display manager required; the installer enables SDDM

## Installation

```bash
sudo pacman -S --needed git base-devel
git clone https://github.com/thatsvile/anomale.git
chmod +x anomale/anomale/install.sh
bash anomale/anomale/install.sh
```

The script asks for sudo early and keeps it alive. It will also ask whether you
have an NVIDIA GPU so the niri session autostart script gets the right
environment. When it finishes, reboot.

What the installer roughly does: installs pacman packages (including niri), a few Python tools
via pip, builds Anomale, copies configs and wallpapers, sets up SDDM, and wires session autostart.

## Essential keybinds

From `~/.config/niri/config.kdl` after install. Super is the Windows/Command key.

| Binding | Action |
| --- | --- |
| `Super` + `Tab` | Terminal (`foot`) |
| `Super` + `q` | Close focused window |
| `Alt` + `Space` | App launcher |
| `Super` + `Space` | Power menu |
| `Super` + `Shift` + `l` | Wallpaper picker (updates pywal) |
| `Super` + `Shift` + `t` | System tray |
| `Alt` + arrows | Move focus |
| `Super` + `Left` / `Right` | Switch tags |
| `Alt` + `f` | Fullscreen |
| `Super` + `a` | Toggle floating |
| `Alt` + `Tab` | Overview |

The full bind list is in the niri config — edit it there.

Useful terminal popups bound in the same file: wifi (`wifitui`), `btop`,
`pulsemixer`, screenshots / short recordings (`mangoshooter` / `mangorecorder`).

## After install

- Log in through SDDM (Anomalous theme).
- Default terminal is `foot`; shell is `fish`.
- Wallpapers live in `~/Pictures/wallpaper/`. Picking one through Anomale
  refreshes pywal colors for terminal, GTK, niri, and the SDDM background.
- **niri:** `~/.config/niri/`
- **Anomale:** `~/.config/anomale/` (`config.conf`, `menus.conf`, `notifications.conf`)
- **pywal templates:** `~/.config/wal/templates/`

## Maintenance (`anomale-apps`)

These dots intentionally avoid the AUR. Anything that used to come from there
(or otherwise is not in the official Arch repos) is installed from trusted
upstream sources instead. After install, `anomale-apps` in `~/.local/bin` is
how you keep those pieces current without re-running the full installer.

```bash
anomale-apps status   # installed vs upstream, no changes
anomale-apps update   # update what is behind
```

It tracks:

- **Anomale shell** (rebuild from this repo’s `shell/` tree → `~/.local/bin/anomale`)
- **LibreWolf** and **niri** (official Arch packages; LibreWolf profiles in `~/.librewolf` are left alone)
- **wifitui** (GitHub releases)
- **pip:** pywal16, pywalfox, haishoku, colorz

Regular Arch packages still update with `pacman` as usual. Use `anomale-apps`
for the non-repo stack above.

## Layout of this repo

```
anomale/
  install.sh              # primary install path
  thestuff/
    shell/                # Anomale Shell (Rust)
    .config/              # shipped user configs
    .local/bin/           # helpers (incl. anomale-apps) and session scripts
    anomalous/            # SDDM theme
    wallpaper/            # starter wallpapers
    pacmanlist.txt        # official packages the installer pulls
```

## Notes

This repo tracks my machines. Expect breakage if you follow it blindly.
Issues and patches may sit unanswered for a long time, or forever.
