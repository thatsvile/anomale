# Anomale

[![Watch the example video](https://img.youtube.com/vi/_Iyf3RlilNw/maxresdefault.jpg)](https://www.youtube.com/watch?v=_Iyf3RlilNw)

Personal Arch Linux / Debian Sid dots and a small Wayland shell, built around
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
| Arch or Debian Trixie | Base system (`install.sh` or `debianinstall.sh`) |
| niri | Window manager / compositor |
| Anomale | Bar, menus, notifications, tray, wallpaper → pywal |
| pywal16 | Color scheme from wallpaper (terminal, GTK, niri, SDDM, browser) |
| SDDM + Anomalous | Display manager and login theme |
| foot + fish | Default terminal and shell |

## Requirements

- Fresh **Arch** install, or minimal **Debian Trixie** (Forky/Sid may work)
- Working network (`pacman` mirrors on Arch; apt on Debian)
- No existing DE or display manager required; the installer enables SDDM

## Installation

### Arch

```bash
sudo pacman -S --needed git base-devel
git clone https://github.com/thatsvile/anomale.git
chmod +x anomale/anomale/install.sh
bash anomale/anomale/install.sh
```

### Debian Trixie

```bash
sudo apt-get update
sudo apt-get install -y git
git clone https://github.com/thatsvile/anomale.git
git -C anomale checkout deb
chmod +x anomale/anomale/debianinstall.sh
bash anomale/anomale/debianinstall.sh
```

Both scripts ask for sudo early and keep it alive. They also ask whether you
have an NVIDIA GPU so the niri session autostart script gets the right
environment. When either finishes, reboot.

**Arch** installer: pacman packages (including niri), pip tools, builds Anomale,
copies configs/wallpapers, sets up SDDM.

**Debian** installer (`debianinstall.sh` on the `deb` branch): enables LibreWolf
via `extrepo`, on Trixie installs a `trixie-backports` kernel if you are not
already on one, installs apt packages from `debpackagelist.txt`, builds niri
and xwayland-satellite from upstream git, installs adw-gtk3 for GTK theming,
then the same Anomale/dots/SDDM path. On NVIDIA **YES**, it asks GPU generation,
adds NVIDIA’s CUDA apt repos, and installs drivers (GTX 10xx / Pascal → 580
proprietary from the debian12 CUDA repo, pinned; RTX 20xx+ → newest `nvidia-open`
from debian13, unpinned). Arch helper scripts under `thestuff/` are left
unchanged; Debian-specific helpers are separate files.

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

On Debian (`debianinstall.sh`), `~/.local/bin/anomale-apps` is the Debian variant from `thestuff/debian/anomale-apps`. It also rebuilds **niri** and **xwayland-satellite** from upstream git into `/usr/local/bin`. Arch still uses `thestuff/.local/bin/anomale-apps` (no git rebuild of niri).

Regular Arch packages still update with `pacman` as usual. Use `anomale-apps`
for the non-repo stack above.

## Layout of this repo

```
anomale/
  install.sh              # Arch install path
  debianinstall.sh        # Debian Sid/Forky install path
  thestuff/
    shell/                # Anomale Shell (Rust)
    .config/              # shipped user configs
    .local/bin/           # helpers (incl. anomale-apps) and session scripts
    debian/               # Debian-only helpers (Debian anomale-apps, etc.)
    anomalous/            # SDDM theme
    wallpaper/            # starter wallpapers
    pacmanlist.txt        # Arch packages (install.sh)
    debpackagelist.txt    # Debian packages (debianinstall.sh)
```

## Notes

This repo tracks my machines. Expect breakage if you follow it blindly.
Issues and patches may sit unanswered for a long time, or forever.
