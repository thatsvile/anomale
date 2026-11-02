# vile

[![Watch the example video](https://img.youtube.com/vi/_Iyf3RlilNw/maxresdefault.jpg)](https://www.youtube.com/watch?v=_Iyf3RlilNw)

Personal Arch Linux dots and [viledesktop](viledesktop/), built around
[niri](https://github.com/YaLTeR/niri) and vilecolors.

This is software I maintain for my own machines. You can use it if you want.
It is not a general-purpose desktop, and it is not written with support in mind.

## Overview

Two pieces that ship together:

1. **Dotfiles** — niri session config, terminal/shell setup (`foot` / `fish`),
   GTK theming, SDDM theme, and color templates that keep the session consistent.
2. **viledesktop** — a thin Wayland shell on top of niri: text bar, app search,
   power menu, notifications, tray, wallpaper, screenshots, and vilecolors /
   vilefox theming.

It does not try to replace a full desktop environment. No applet pile, no
heavily customized GUI apps. The point is to stay out of the way and keep you
in the terminal as much as possible. If that sounds annoying, this setup is
not for you — that is intentional.

Sources for the shell live under `viledesktop/`. Day to day you are not meant
to build that by hand; the install script builds it and installs the dots.

## Stack

| Piece | Role |
| --- | --- |
| Arch Linux | Base system (official repos + a few upstream builds; no AUR) |
| niri | Window manager / compositor |
| viledesktop | Bar, menus, notifications, tray, wallpaper, screenshots |
| vilecolors | Color scheme from wallpaper (terminal, GTK, niri, SDDM) |
| vilefox | LibreWolf / Firefox theme from the same palette |
| SDDM + vile | Display manager and login theme |
| foot + fish | Default terminal and shell |

## Requirements

- Fresh Arch install (CachyOS / EndeavourOS may work; recent versions untested)
- Working network and a usable `pacman` mirrorlist
- No existing DE or display manager required; the installer enables SDDM

## Installation

```bash
sudo pacman -S --needed git base-devel
git clone https://github.com/thatsvile/anomale.git
chmod +x anomale/vile/install.sh
bash anomale/vile/install.sh
```

The script asks for sudo early and keeps it alive. It will also ask whether you
have an NVIDIA GPU so the niri session autostart script gets the right
environment. When it finishes, reboot.

What the installer roughly does: installs pacman packages (including niri),
builds viledesktop, copies configs and wallpapers, seeds a vilecolors palette,
sets up SDDM, installs the vilefox native host, and wires session autostart.

## Essential keybinds

From `~/.config/niri/config.kdl` after install. Super is the Windows/Command key.

| Binding | Action |
| --- | --- |
| `Super` + `Tab` | Terminal (`foot`) |
| `Super` + `q` | Close focused window |
| `Alt` + `Space` | App launcher |
| `Super` + `Space` | Power menu |
| `Super` + `Shift` + `t` | System tray |
| `Super` + `n` | Notifications |
| `Super` + `Shift` + `b/m/c/g` | Battery / mem / cpu / gpu overlays |
| `Super` + `Shift` + `p/o` | Screenshot full / crop |
| `Alt` + arrows | Move focus |
| `Super` + `Left` / `Right` | Switch tags |
| `Alt` + `f` | Fullscreen |
| `Super` + `a` | Toggle floating |
| `Alt` + `Tab` | Overview |

The full bind list is in the niri config — edit it there.

Useful terminal popups bound in the same file: wifi (`wifitui`), `pulsemixer`.

## After install

- Log in through SDDM (vile theme).
- Default terminal is `foot`; shell is `fish`.
- Wallpapers live in `~/Pictures/wallpaper/`. Applying colors through
  `vile --colors` refreshes terminal, GTK, niri, SDDM, and LibreWolf (vilefox).
  Post-apply shell commands live in `~/.config/vile/config.json` under
  `colors_apply_commands` (niri reload + `vile-refresh.sh` by default).
  SDDM sync goes through a polkit helper (`/usr/local/lib/vile/sddm-sync.sh`);
  the theme directory stays root-owned and is not user-writable.
- **niri:** `~/.config/niri/`
- **vile:** `~/.config/vile/` (`config.json`, recipes, templates, vilefox.json)
- **templates:** `~/.config/wal/templates/` and `~/.config/vile/colors/templates/`

LibreWolf auto-installs the signed vilefox add-on via policies
(`normal_installed` from `/usr/local/share/vile/vilefox-signed.xpi`). Firefox
still needs the XPI opened once (`~/.local/share/vilefox/vilefox-signed.xpi`
after `vilefox install`). Disable pywalfox if it is still installed.

## Maintenance (`vile-apps`)

These dots intentionally avoid the AUR. Anything that used to come from there
(or otherwise is not in the official Arch repos) is installed from trusted
upstream sources instead. After install, `vile-apps` in `~/.local/bin` is
how you keep those pieces current without re-running the full installer.

```bash
vile-apps status   # installed vs upstream, no changes
vile-apps update   # update what is behind
```

It tracks:

- **viledesktop** (rebuild from this repo’s `viledesktop/` tree → `~/.local/bin`)
- **wifitui** (GitHub releases)

Regular Arch packages still update with `pacman` as usual. Use `vile-apps`
for the non-repo stack above.

## Layout of this repo

```
vile/
  install.sh              # primary install path
  thestuff/
    .config/              # shipped user configs (incl. vile + niri)
    .local/bin/           # helpers (incl. vile-apps) and session scripts
    vile/                 # SDDM theme (root-owned after install)
    usr/local/lib/vile/   # privileged helpers (sddm-sync, splash, …)
    wallpaper/            # starter wallpapers
    pacmanlist.txt        # official packages the installer pulls
viledesktop/              # viledaemon, vilebar, vilecolors, vilewall, …
```

## Notes

This repo tracks my machines. Expect breakage if you follow it blindly.
Issues and patches may sit unanswered for a long time, or forever.


**AI Disclosure**

AI has been used as part of the creation process for aspects of this project.
