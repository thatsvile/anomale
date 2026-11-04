#!/bin/bash
set -Eeuo pipefail

clear
SCRIPT_DIR=$( cd -- "$( dirname -- "${BASH_SOURCE[0]}" )" &> /dev/null && pwd )
THE_STUFF="$SCRIPT_DIR/thestuff"
SUDOERS_DROPIN="/etc/sudoers.d/99-vile-install"
BUILD_ROOT=""

export PATH="$PATH:/usr/local/bin:$HOME/.local/bin"

report_failure() {
    local exit_code=$?
    [[ "$BASHPID" == "$$" ]] || return 0
    echo "" >&2
    echo "ERROR: install.sh aborted at line ${1} (exit ${exit_code})." >&2
    echo "Failed command: ${2}" >&2
}
trap 'report_failure "$LINENO" "$BASH_COMMAND"' ERR

cleanup_install() {
    if [[ -f "$SUDOERS_DROPIN" ]]; then
        sudo rm -f "$SUDOERS_DROPIN" 2>/dev/null || true
    fi
    if [[ -n "$BUILD_ROOT" && -d "$BUILD_ROOT" ]]; then
        rm -rf "$BUILD_ROOT" 2>/dev/null || true
    fi
}
trap cleanup_install EXIT INT TERM

rewrite_shipped_home_paths() {
    local dest="$1"
    [[ -d "$dest" ]] || return 0
    local f
    while IFS= read -r f; do
        sed -i "s|/home/jor|${HOME}|g; s|__VILE_HOME__|${HOME}|g" "$f"
    done < <(grep -rlI -e '/home/jor' -e '__VILE_HOME__' "$dest" 2>/dev/null || true)
}

detect_cpu_arch() {
    case "$(uname -m)" in
        x86_64) echo "x86_64" ;;
        aarch64|arm64) echo "arm64" ;;
        *)
            echo "ERROR: Unsupported architecture: $(uname -m)" >&2
            exit 1
            ;;
    esac
}

enable_multilib() {
    if grep -qE '^\[multilib\]' /etc/pacman.conf; then
        echo "[multilib] already enabled."
        return 0
    fi
    echo "Enabling [multilib] (required for Steam)..."
    local tmp bak
    tmp=$(mktemp)
    bak="/etc/pacman.conf.bak.vile-$(date +%Y%m%d%H%M%S)"
    sudo cp -a /etc/pacman.conf "$bak"
    awk '
      /^#\[multilib\]$/ {
        print "[multilib]"
        getline
        if ($0 ~ /^#Include = \/etc\/pacman\.d\/mirrorlist$/) {
          print "Include = /etc/pacman.d/mirrorlist"
        } else {
          print
        }
        next
      }
      { print }
    ' /etc/pacman.conf >"$tmp"
    sudo cp "$tmp" /etc/pacman.conf
    rm -f "$tmp"
    if ! grep -qE '^\[multilib\]' /etc/pacman.conf; then
        echo "ERROR: could not enable [multilib] in /etc/pacman.conf (backup: $bak)."
        exit 1
    fi
    echo "Syncing package databases after enabling [multilib]..."
    sudo pacman -Sy
}

install_pacman_packages() {
    enable_multilib
    echo "Installing official repository packages..."
    sudo pacman -S --needed --noconfirm - < "$THE_STUFF/pacmanlist.txt"
}

remove_stale_pywal_packages() {
    echo "Removing stale pywal/pywalfox pip packages if present..."
    local stale
    stale=$(pip list --format=freeze 2>/dev/null | cut -d= -f1 \
        | grep -ix -e pywal16 -e pywal -e pywalfox -e haishoku -e colorz || true)
    if [[ -n "$stale" ]]; then
        # shellcheck disable=SC2086
        sudo pip uninstall --break-system-packages -y $stale || true
    fi
    stale=$(pip list --user --format=freeze 2>/dev/null | cut -d= -f1 \
        | grep -ix -e pywal16 -e pywal -e pywalfox -e haishoku -e colorz || true)
    if [[ -n "$stale" ]]; then
        # shellcheck disable=SC2086
        pip uninstall --break-system-packages -y $stale || true
    fi
}

install_getnf() {
    if command -v getnf >/dev/null 2>&1; then
        echo "getnf already installed."
        return 0
    fi
    echo "Installing getnf from upstream..."
    local tmp
    tmp=$(mktemp -d)
    curl -fsSL "https://raw.githubusercontent.com/getnf/getnf/main/getnf" -o "$tmp/getnf"
    sudo install -Dm755 "$tmp/getnf" /usr/local/bin/getnf
    rm -rf "$tmp"
}

install_wifitui() {
    if command -v wifitui >/dev/null 2>&1; then
        echo "wifitui already installed."
        return 0
    fi
    echo "Installing wifitui from upstream release..."
    local tmp tag arch asset url stamp_dir
    tmp=$(mktemp -d)
    arch=$(detect_cpu_arch)
    tag=$(curl -fsSL "https://api.github.com/repos/shazow/wifitui/releases/latest" \
        | python3 -c "import sys,json; print(json.load(sys.stdin)['tag_name'])")
    asset="wifitui-${tag#v}-linux-${arch}.tar.gz"
    url="https://github.com/shazow/wifitui/releases/download/${tag}/${asset}"
    curl -fsSL "$url" -o "$tmp/$asset"
    tar -xzf "$tmp/$asset" -C "$tmp"
    local bin
    bin=$(find "$tmp" -type f -name wifitui -print -quit)
    if [[ -z "$bin" ]]; then
        echo "ERROR: wifitui binary missing from release archive."
        exit 1
    fi
    sudo install -Dm755 "$bin" /usr/local/bin/wifitui
    stamp_dir="${XDG_CACHE_HOME:-$HOME/.cache}/vile/src"
    mkdir -p "$stamp_dir"
    echo "$tag" >"$stamp_dir/wifitui.installed"
    rm -rf "$tmp"
}

echo -e "\033[0;32m" 
cat << "EOF"

Oh wow, I guess you're trying to install...

 __      ___ _     _____
 \ \    / (_) |   | ____|
  \ \  / / _| |   |  _|
   \ \/ / | | |___| |___
    \__/  |_|_____|_____|  desktop + dots

Nice...
EOF
sleep 1
echo -e "\033[0m" 

sleep 0.5

cat << "EOF"
These dots sit on niri with viledesktop: a thin text bar, menus, tray,
notifications, wallpaper, and vilecolors theming (templates, GTK, foot,
niri, SDDM, LibreWolf via vilefox). No widget pile. The point is to stay
out of the way and keep you in the terminal.

The install script builds viledesktop from the sibling viledesktop/
tree, installs prerequisites, and copies configs. Meant for a minimal
Arch install with no DE (official packages + a few trusted upstreams —
no yay/AUR).

After considering all of this, you may proceed.
EOF
sleep 0.3

#are you even ready?
cat << "EOF"
Are you Ready to Install viledesktop and Vile's Dots?
EOF

PS3="Choose (but don't be a coward): "
options=("LETS DO THIS" "GET ME OUTTA HERE")

select opt in "${options[@]}"
do
    case $opt in
        "LETS DO THIS")
            echo "Nice..."
            sleep 1
            break 
            ;;
        "GET ME OUTTA HERE")
            echo "Safe choice. No changes were made. Exiting..."
            sleep 1
            exit 0 
            ;;
        *) 
            echo "Invalid entry. Please pick 1 or 2."
            ;;
    esac
done
clear

#initialization - perms request
echo "Starting the installation..."
echo "You may be asked for your password once or twice; the script keeps sudo alive afterward."
sleep 2
sudo -v

# Keep sudo valid for the whole install without re-prompting constantly.
echo "Defaults:${USER} timestamp_timeout=180" | sudo tee "$SUDOERS_DROPIN" >/dev/null
sudo chmod 440 "$SUDOERS_DROPIN"
sudo visudo -cf "$SUDOERS_DROPIN" >/dev/null

while true; do 
    sudo -n true
    sleep 60
    kill -0 "$$" || exit
done 2>/dev/null &

clear
echo "Checking network and mirrors..."
if ! curl -fsSL --connect-timeout 8 -o /dev/null https://archlinux.org/; then
    echo "ERROR: No network reachability to archlinux.org. Configure network and try again."
    exit 1
fi
if ! grep -qE '^[[:space:]]*Server[[:space:]]*=' /etc/pacman.d/mirrorlist; then
    echo "ERROR: /etc/pacman.d/mirrorlist has no active Server entries. Configure mirrors and try again."
    exit 1
fi

echo "Ensuring git, base-devel, and curl are installed..."
sudo pacman -S --needed --noconfirm git base-devel curl

BUILD_ROOT=$(mktemp -d)

install_pacman_packages
remove_stale_pywal_packages
install_getnf
install_wifitui

#set fish as system-wide shell and set local/bin path.
sudo chsh -s /usr/bin/fish "$USER"

# Required to build viledesktop
rustup default stable
if [[ -f "$HOME/.cargo/env" ]]; then
    # shellcheck disable=SC1091
    source "$HOME/.cargo/env"
fi
if ! command -v cargo >/dev/null 2>&1; then
    echo "ERROR: cargo is not on PATH after rustup. Open a new shell or check rustup install, then re-run."
    exit 1
fi

VILE_ROOT="$SCRIPT_DIR/../viledesktop"
if [[ ! -f "$VILE_ROOT/Cargo.toml" ]]; then
    echo "ERROR: viledesktop workspace missing at $VILE_ROOT"
    exit 1
fi

echo "Building viledesktop..."
(cd "$VILE_ROOT" && cargo build --release --workspace)
mkdir -p "$HOME/.local/bin/"
for bin in vile viledaemon vilebar vilecolors vilewall vileshot vilefox; do
    if [[ ! -x "$VILE_ROOT/target/release/$bin" ]]; then
        echo "ERROR: viledesktop failed to build (missing target/release/$bin)."
        exit 1
    fi
    cp "$VILE_ROOT/target/release/$bin" "$HOME/.local/bin/$bin"
    chmod +x "$HOME/.local/bin/$bin"
done

mkdir -p "$HOME/.local/share/"
mkdir -p "$HOME/.local/share/icons/"

git clone https://aur.archlinux.org/papirus-folders-git.git "$THE_STUFF/papirus-folders-git/"
(cd "$THE_STUFF/papirus-folders-git/" && makepkg -si)

cp -a /usr/share/icons/Papirus "$HOME/.local/share/icons/Vile-Papirus"

sed -i 's/^Name=.*/Name=Vile-Papirus/' "$HOME/.local/share/icons/Vile-Papirus/index.theme"

# Stamp viledesktop tree so vile-apps update can skip rebuild when unchanged.
stamp_dir="${XDG_CACHE_HOME:-$HOME/.cache}/vile/src"
mkdir -p "$stamp_dir"
if git -C "$SCRIPT_DIR/.." rev-parse --is-inside-work-tree >/dev/null 2>&1; then
    git -C "$SCRIPT_DIR/.." rev-parse --short=12 HEAD:viledesktop \
        >"$stamp_dir/viledesktop.installed" 2>/dev/null || true
fi

# copy wallpaper / .config / .local
mkdir -p "$HOME/Pictures/wallpaper/"
cp -r "$THE_STUFF/wallpaper/." "$HOME/Pictures/wallpaper/"

mkdir -p "$HOME/.config/"
cp -r "$THE_STUFF/.config/." "$HOME/.config/"

rewrite_shipped_home_paths "$HOME/.config"

mkdir -p "$HOME/Misc"
mkdir -p "$HOME/Pictures"
mkdir -p "$HOME/Downloads"
mkdir -p "$HOME/Videos"

cat << EOF > "$HOME/.config/gtk-3.0/bookmarks"
file://$HOME/Misc Misc
file://$HOME/Downloads Downloads
file://$HOME/Pictures Pictures
file://$HOME/Videos Videos
EOF

mkdir -p "$HOME/.local/bin/"
cp -r "$THE_STUFF/.local/bin/." "$HOME/.local/bin/"
chmod +x "$HOME/.local/bin/"*

# nwg-look apply DB + export so GTK/xsettings/gtkrc match Vile-Papirus defaults.
mkdir -p "$HOME/.local/share/nwg-look"
cp -f "$THE_STUFF/.local/share/nwg-look/gsettings" "$HOME/.local/share/nwg-look/gsettings"
if command -v nwg-look >/dev/null 2>&1; then
    nwg-look -a
    nwg-look -x
fi
gsettings set org.gnome.desktop.interface icon-theme 'Vile-Papirus'
gsettings set org.gnome.desktop.interface gtk-theme 'adw-gtk3-dark'
gsettings set org.gnome.desktop.interface color-scheme 'prefer-dark'

# Ensure NVIDIA/non-NVIDIA start scripts are present before the GPU prompt.
if [[ ! -f "$HOME/.local/bin/niri-start-nvidia.sh" || ! -f "$HOME/.local/bin/niri-start-nonvidia.sh" ]]; then
    echo "ERROR: niri start scripts missing from ~/.local/bin after copy."
    exit 1
fi

#set terminal
fish -c "set -Ux TERMINAL foot"

# Seed colors cache + ~/.cache/wal compat so GTK symlinks and SDDM sync have targets.
SEED_WALL=""
for candidate in \
    "$HOME/Pictures/wallpaper/wallpaper_2.png" \
    "$HOME/Pictures/wallpaper/"*.png \
    "$HOME/Pictures/wallpaper/"*.jpg \
    "$HOME/Pictures/wallpaper/"*.jpeg; do
    if [[ -f "$candidate" ]]; then
        SEED_WALL="$candidate"
        break
    fi
done
if [[ -z "$SEED_WALL" ]]; then
    echo "ERROR: no seed wallpaper found under ~/Pictures/wallpaper/"
    exit 1
fi
if [[ ! -f "$HOME/.config/vile/colors/palettes/default.json" ]]; then
    echo "ERROR: missing shipped vilecolors recipe default.json"
    exit 1
fi
echo "Applying initial vilecolors palette from $(basename "$SEED_WALL")..."
# Skip colors_apply_commands* during seed (niri/wal-refresh are not useful mid-install).
python3 - <<'PY'
import json
from pathlib import Path
p = Path.home() / ".config/vile/config.json"
cfg = json.loads(p.read_text())
cfg["_install_saved_colors_apply_commands"] = cfg.get("colors_apply_commands", [])
cfg["_install_saved_colors_apply_commands_dark"] = cfg.get("colors_apply_commands_dark", [])
cfg["_install_saved_colors_apply_commands_light"] = cfg.get("colors_apply_commands_light", [])
cfg["colors_apply_commands"] = []
cfg["colors_apply_commands_dark"] = []
cfg["colors_apply_commands_light"] = []
p.write_text(json.dumps(cfg, indent=2) + "\n")
PY
"$HOME/.local/bin/vilecolors" --recipe default --img "$SEED_WALL"
python3 - <<'PY'
import json
from pathlib import Path
p = Path.home() / ".config/vile/config.json"
cfg = json.loads(p.read_text())
_vesktop_cp = "cp ~/.cache/wal/vesktop-clienttheme.css ~/.var/app/dev.vencord.Vesktop/config/vesktop/settings/quickCss.css"
cfg["colors_apply_commands"] = cfg.pop("_install_saved_colors_apply_commands", [
    "niri msg action load-config-file",
    "~/.local/bin/vile-refresh.sh",
    _vesktop_cp,
])
cfg["colors_apply_commands_dark"] = cfg.pop("_install_saved_colors_apply_commands_dark", [
    "niri msg action load-config-file",
    "~/.local/bin/vile-refresh.sh dark",
    _vesktop_cp,
])
cfg["colors_apply_commands_light"] = cfg.pop("_install_saved_colors_apply_commands_light", [
    "niri msg action load-config-file",
    "~/.local/bin/vile-refresh.sh light",
    _vesktop_cp,
])
p.write_text(json.dumps(cfg, indent=2) + "\n")
PY
# Persist wallpaper state so first-boot vile --refresh / vilewall restores an image.
SEED_WALL="$SEED_WALL" python3 - <<'PY'
import json
import os
from pathlib import Path
home = Path.home()
cfg = json.loads((home / ".config/vile/config.json").read_text())
mode = cfg.get("wallpaper", {}).get("mode", "fill")
img = Path(os.environ["SEED_WALL"]).expanduser().resolve()
if not img.is_file():
    raise SystemExit(f"seed wallpaper missing: {img}")
path = home / ".cache" / "vile" / "wallpaper.json"
path.parent.mkdir(parents=True, exist_ok=True)
path.write_text(json.dumps({"image": str(img), "mode": mode}, indent=2) + "\n")
print(f"Seeded wallpaper state from {img.name} (mode={mode})")
PY
"$HOME/.local/bin/vilefox" install

# Seed Papirus folder colors now that wal cache exists (skipped during colors seed).
if [[ -x "$HOME/.local/bin/folder-col.sh" ]]; then
    bash "$HOME/.local/bin/folder-col.sh" || true
fi

# gtk wal-compat symlinks (vilecolors writes ~/.cache/vile/wal and links ~/.cache/wal)
rm -f "$HOME/.config/gtk-4.0/gtk.css"
rm -f "$HOME/.config/gtk-4.0/gtk-dark.css"
rm -f "$HOME/.config/gtk-3.0/gtk.css"
rm -f "$HOME/.config/gtk-3.0/gtk-dark.css"

ln -s "$HOME/.cache/wal/gtk-css.css" "$HOME/.config/gtk-4.0/gtk.css"
ln -s "$HOME/.cache/wal/gtk-css.css" "$HOME/.config/gtk-4.0/gtk-dark.css"
ln -s "$HOME/.cache/wal/gtk-css.css" "$HOME/.config/gtk-3.0/gtk.css"
ln -s "$HOME/.cache/wal/gtk-css.css" "$HOME/.config/gtk-3.0/gtk-dark.css"

#install font
echo "Installing 0xProto Nerd Font..."
getnf -i "0xProto"
if ! getnf -l 2>/dev/null | grep -qi '0xProto'; then
    if ! find "$HOME/.local/share/fonts" "$HOME/.fonts" -iname '*0xProto*' 2>/dev/null | grep -q .; then
        echo "ERROR: getnf did not install 0xProto."
        exit 1
    fi
fi

#nvidia check
clear
cat << "EOF"
To make sure your environment variables in your autostart script are configured properly, Please Share whether 
or not you suffer from "I have an NVidia GPU and Use Linux" disorder.
EOF


PS3="DO YOU HAVE NVIDIA GPU?: "
options=("YES" "NO")

select opt in "${options[@]}"
do
    case $opt in
        "YES")
            echo "sorry..."
            rm -f "$HOME/.local/bin/niri-start-nonvidia.sh"
            mv "$HOME/.local/bin/niri-start-nvidia.sh" "$HOME/.local/bin/niri-start.sh"
            echo "Installing 32-bit NVIDIA libs for Steam/Proton..."
            sudo pacman -S --needed --noconfirm lib32-nvidia-utils
            sleep 1
            break 
            ;;
        "NO")
            echo "lucky..."
            rm -f "$HOME/.local/bin/niri-start-nvidia.sh"
            mv "$HOME/.local/bin/niri-start-nonvidia.sh" "$HOME/.local/bin/niri-start.sh"
            sleep 1
            break
            ;;
        *) 
            echo "Invalid entry. Please pick 1 or 2."
            ;;
    esac
done
chmod +x "$HOME/.local/bin/"*
clear

echo "Configuring SDDM, theme, boot splash, and LibreWolf policies..."
sudo bash -c "
set -euo pipefail
systemctl enable sddm
systemctl set-default graphical.target
install -d /usr/share/sddm/themes /usr/local/lib/vile /etc/sddm.conf.d
install -d /usr/share/polkit-1/actions /etc/polkit-1/rules.d
cp -r \"$THE_STUFF/vile\" /usr/share/sddm/themes/
# Keep theme root-owned; live colors/wallpaper are written by sddm-sync.sh.
chown -R root:root /usr/share/sddm/themes/vile
install -m 644 \"$THE_STUFF/etc/sddm.conf.d/10-vile.conf\" /etc/sddm.conf.d/10-vile.conf
install -m 755 \"$THE_STUFF/usr/local/lib/vile/sddm-sync.sh\" /usr/local/lib/vile/sddm-sync.sh
install -m 644 \"$THE_STUFF/usr/share/polkit-1/actions/com.vile.sddm.sync.policy\" /usr/share/polkit-1/actions/com.vile.sddm.sync.policy
install -m 644 \"$THE_STUFF/etc/polkit-1/rules.d/10-vile-sddm.rules\" /etc/polkit-1/rules.d/10-vile-sddm.rules
# Seed greeter from the installing user's vile/wal cache.
SUDO_USER=\"$USER\" SUDO_UID=\"$(id -u)\" /usr/local/lib/vile/sddm-sync.sh
if [[ -f \"$THE_STUFF/splash-arch.bmp\" ]]; then
    install -d /usr/local/share/vile /usr/local/lib/vile /etc/pacman.d/hooks
    install -m 644 \"$THE_STUFF/splash-arch.bmp\" /usr/local/share/vile/splash.bmp
    install -m 755 \"$THE_STUFF/usr/local/lib/vile/restore-splash.sh\" /usr/local/lib/vile/restore-splash.sh
    install -m 644 \"$THE_STUFF/etc/pacman.d/hooks/zz-vile-splash.hook\" /etc/pacman.d/hooks/zz-vile-splash.hook
    rm -f /etc/pacman.d/hooks/vile-splash.hook
    /usr/local/lib/vile/restore-splash.sh --force-rebuild

fi
if [[ -f \"$THE_STUFF/etc/librewolf/policies.json\" ]]; then
    install -d /usr/local/share/vile /usr/local/lib/vile /etc/pacman.d/hooks
    install -m 644 \"$THE_STUFF/etc/librewolf/policies.json\" /usr/local/share/vile/librewolf-policies.json
    install -m 755 \"$THE_STUFF/usr/local/lib/vile/librewolf-policy.sh\" /usr/local/lib/vile/librewolf-policy.sh
    install -m 644 \"$THE_STUFF/etc/pacman.d/hooks/vile-librewolf-policy.hook\" /etc/pacman.d/hooks/vile-librewolf-policy.hook
    # Stable path for ExtensionSettings install_url (also staged under ~/.local/share/vilefox).
    if [[ -f \"$HOME/.local/share/vilefox/vilefox-signed.xpi\" ]]; then
        install -m 644 \"$HOME/.local/share/vilefox/vilefox-signed.xpi\" /usr/local/share/vile/vilefox-signed.xpi
    elif [[ -f \"$VILE_ROOT/vilefox/vilefox-signed.xpi\" ]]; then
        install -m 644 \"$VILE_ROOT/vilefox/vilefox-signed.xpi\" /usr/local/share/vile/vilefox-signed.xpi
    fi
    /usr/local/lib/vile/librewolf-policy.sh
fi
"

fish << EOF
if not contains "$HOME/.local/bin" \$fish_user_paths
    set -Ua fish_user_paths "$HOME/.local/bin"
end
set -Ux TERMINAL foot
EOF
clear

mkdir -p "$HOME/.config/xdg-desktop-portal-wlr"
cat << 'EOF' > "$HOME/.config/xdg-desktop-portal-wlr/wlroots"
[screencast]
chooser_cmd=slurp -f %o -or
chooser_type=simple
EOF

cat << "EOF"
This is the end of the script. Please reboot your computer.
EOF
