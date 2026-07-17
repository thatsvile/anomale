# Anomale Shell + Dots

[![Watch the example video](https://i.postimg.cc/28XHGczx/image-2.png)](https://www.youtube.com/watch?v=IXHZVE5SDYE)

Personal Arch Linux dotfiles and a small Wayland graphical shell, built around [MangoWM](https://mangowm.github.io) and pywal. This is software I maintain for my own machines. You can use it if you want, but it is not a general-purpose desktop and it is not written with support in mind.

## What this is

Anomale Shell is a thin interface on top of MangoWM: a bar, an app launcher, a power menu, notifications, and a wallpaper picker that drives pywal theming. It does not try to replace a full desktop environment. There is no pile of applet widgets or heavily customized GUI apps. The point is to stay out of the way and keep you in the terminal as much as possible.

The Rust sources for the shell live under `anomale/thestuff/shell/`. Day to day you are not meant to build that by hand. The install script takes a minimal Arch install (no DE, no display manager), bootstraps yay, pulls dependencies, builds Anomale, drops in the configs, sets up SDDM with the included theme, and wires the boot splash so it survives package upgrades.

If you are not comfortable living in a terminal and editing config files, this setup will probably annoy you. That is intentional.

## Requirements

- Fresh Arch Linux install (Arch, CachyOS, or EndeavourOS base-style installs are the intended target)
- Working network and a usable `pacman` mirrorlist
- No existing desktop environment or display manager required; the installer enables SDDM

## Installation

```bash
sudo pacman -S --needed git base-devel
git clone https://github.com/thatsvile/anomale.git
chmod +x anomale/anomale/install.sh
bash anomale/anomale/install.sh
```

The script will ask for sudo early and keep it alive for the rest of the run. It also asks whether you have an NVIDIA GPU so the Mango session autostart script gets the right environment variables. When it finishes, reboot.

## Essential keybinds

These come from `~/.config/mango/config.conf` after install. Super is the Windows/Command key.

| Binding | Action |
| --- | --- |
| `Super` + `Tab` | Open a terminal (`foot`) |
| `Super` + `q` | Close the focused window |
| `Alt` + `Space` | App launcher |
| `Super` + `Space` | Power menu (shutdown / reboot / logout) |
| `Super` + `Shift` + `l` | Wallpaper picker (updates pywal theme) |

A few more that are useful right away:

| Binding | Action |
| --- | --- |
| `Alt` + arrow keys | Move focus between tiled windows |
| `Super` + `Left` / `Right` | Switch tags |
| `Alt` + `f` | Toggle fullscreen |
| `Super` + `a` | Toggle floating |
| `Alt` + `Tab` | Overview |

The full bind list lives in the mango config. Change it there if you want different muscle memory.

## After install

- Session entry is SDDM with the included Anomalous theme.
- Default terminal is `foot`; shell is `fish`.
- Wallpapers land in `~/Pictures/wallpaper/`. Changing wallpaper through Anomale refreshes pywal colors across terminal, GTK, and the SDDM theme background.
- Anomale and Mango configs live under `~/.config/anomale/` and `~/.config/mango/`.

## Notes

This repo will keep changing as I adjust my own setup. Expect breakage if you track it blindly. Issues and patches from other people may sit unanswered for a long time, or forever.
