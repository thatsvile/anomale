#!/bin/bash
set -euo pipefail

#clear tty and define variables
clear
SCRIPT_DIR=$( cd -- "$( dirname -- "${BASH_SOURCE[0]}" )" &> /dev/null && pwd )
THE_STUFF="$SCRIPT_DIR/thestuff"
SUDOERS_DROPIN="/etc/sudoers.d/99-anomale-install"

cleanup_install() {
    if [[ -f "$SUDOERS_DROPIN" ]]; then
        sudo rm -f "$SUDOERS_DROPIN" 2>/dev/null || true
    fi
}
trap cleanup_install EXIT INT TERM

# Replace shipped absolute paths so cache/configs work for the installing user.
rewrite_shipped_home_paths() {
    local dest="$1"
    [[ -d "$dest" ]] || return 0
    local f
    while IFS= read -r f; do
        sed -i "s|/home/jor|${HOME}|g; s|__ANOMALE_HOME__|${HOME}|g" "$f"
    done < <(grep -rlI -e '/home/jor' -e '__ANOMALE_HOME__' "$dest" 2>/dev/null || true)
}

#welcome to the installer, kid... Have a disclaimer.
echo -e "\033[0;32m" 
cat << "EOF"

Oh wow, I guess you're trying to install...
 _____                                                  _____ 
( ___ )                                                ( ___ )
 |   |~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~|   | 
 |   |     _    _   _  ___  __  __    _    _     _____  |   | 
 |   |    / \  | \ | |/ _ \|  \/  |  / \  | |   | ____| |   | 
 |   |   / _ \ |  \| | | | | |\/| | / _ \ | |   |  _|   |   | 
 |   |  / ___ \| |\  | |_| | |  | |/ ___ \| |___| |___  |   | 
 |   | /_/__ \_\_|_\_|\___/|_|  |_/_/   \_\_____|_____| |   | 
 |   | / ___|| | | | ____| |   | |                      |   | 
 |   | \___ \| |_| |  _| | |   | |                      |   | 
 |   |  ___) |  _  | |___| |___| |___                   |   | 
 |   | |____/|_| |_|_____|_____|_____| With Vile's Dots!|   | 
 |___|~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~|___| 
(_____)                                                (_____)

Nice...
EOF
sleep 1
echo -e "\033[0m" 

sleep 0.5

cat << "EOF"
Expanding on the philosophy that mangowm offers, anomale shell does **not** include 
a suite of widgets and apps that create a complete desktop environment. 
Instead, it provides a minimal, lightweight, and functional interface 
that provides basic information and wallpaper chooser with 
pywal theming for your minimalistic desktop. 
New features will be added in the future, but the project 
will always maintain that minimalistic philosophy that stays 
out of the user's way and encourages the use of the terminal 
rather than a complicated GUI. Users that are not comfortable working in 
their terminal will likely not enjoy these dots.

While the Anomale Shell source code was included in the 
repo, inside of the shell/ directory, the install script 
is the primary way to install the shell, as it handles the building 
of the binary, installation of any pre-requisites, and the 
copying of configuration files that turn a tedious setup experience 
into a simple 10-minute process.

This Graphical Shell and the included dotfiles 
are meant to be installed over a minimal 
Arch Linux installation with 
no DE or display manager. (The script may work if used under 
different conditions, but no promises. It bootstraps yay and
installs the package list for you.)

After considering all of this, you may proceed.
EOF
sleep 0.3

#are you even ready?
cat << "EOF"
Are you Ready to Install Anomale Shell and Vile's Dots?
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

echo "Ensuring git and base-devel are installed..."
sudo pacman -S --needed --noconfirm git base-devel

if ! command -v yay >/dev/null 2>&1; then
    echo "Bootstrapping yay from the AUR..."
    YAY_TMP=$(mktemp -d)
    git clone https://aur.archlinux.org/yay.git "$YAY_TMP/yay"
    (cd "$YAY_TMP/yay" && makepkg -si --noconfirm)
    rm -rf "$YAY_TMP"
fi

if ! command -v yay >/dev/null 2>&1; then
    echo "ERROR: yay is not available after bootstrap."
    exit 1
fi

echo "Installing packages..."
yay -S --needed --noconfirm --combinedupgrade --sudoloop - < "$THE_STUFF/aurlist.txt"

#set fish as system-wide shell and set local/bin path.
chsh -s /usr/bin/fish

#Required to build anomale
rustup default stable
if [[ -f "$HOME/.cargo/env" ]]; then
    # shellcheck disable=SC1091
    source "$HOME/.cargo/env"
fi
if ! command -v cargo >/dev/null 2>&1; then
    echo "ERROR: cargo is not on PATH after rustup. Open a new shell or check rustup install, then re-run."
    exit 1
fi

#build anomale from source, and copy the binary to ~/.local/bin/ & make executable.
echo "Building Anomale..."
(cd "$THE_STUFF/shell/" && cargo build --release)
if [[ ! -x "$THE_STUFF/shell/target/release/anomale" ]]; then
    echo "ERROR: Anomale failed to build (missing target/release/anomale)."
    exit 1
fi
mkdir -p "$HOME/.local/bin/"
cp "$THE_STUFF/shell/target/release/anomale" "$HOME/.local/bin/"
chmod +x "$HOME/.local/bin/anomale"

#copy wallpaper/ .config .local & .cache contents
mkdir -p "$HOME/Pictures/wallpaper/"
cp -r "$THE_STUFF/wallpaper/." "$HOME/Pictures/wallpaper/"

mkdir -p "$HOME/.cache/"
cp -r "$THE_STUFF/.cache/." "$HOME/.cache/"

mkdir -p "$HOME/.config/"
cp -r "$THE_STUFF/.config/." "$HOME/.config/"

rewrite_shipped_home_paths "$HOME/.cache"
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

# Ensure NVIDIA/non-NVIDIA start scripts are present before the GPU prompt.
if [[ ! -f "$HOME/.local/bin/mangowc-start-nvidia.sh" || ! -f "$HOME/.local/bin/mangowc-start-nonvidia.sh" ]]; then
    echo "ERROR: mangowc start scripts missing from ~/.local/bin after copy."
    exit 1
fi

#set terminal
fish -c "set -Ux TERMINAL foot"

#gtk pywal symlinks
rm -f "$HOME/.config/gtk-4.0/gtk.css"
rm -f "$HOME/.config/gtk-4.0/gtk-dark.css"
rm -f "$HOME/.config/gtk-3.0/gtk.css"
rm -f "$HOME/.config/gtk-3.0/gtk-dark.css"

ln -s "$HOME/.cache/wal/gtk-css.css" "$HOME/.config/gtk-4.0/gtk.css"
ln -s "$HOME/.cache/wal/gtk-css.css" "$HOME/.config/gtk-4.0/gtk-dark.css"
ln -s "$HOME/.cache/wal/gtk-css.css" "$HOME/.config/gtk-3.0/gtk.css"
ln -s "$HOME/.cache/wal/gtk-css.css" "$HOME/.config/gtk-3.0/gtk-dark.css"

#install font and some python packages
echo "Installing 0xProto Nerd Font..."
getnf -i "0xProto"
if ! getnf -l 2>/dev/null | grep -qi '0xProto'; then
    if ! find "$HOME/.local/share/fonts" "$HOME/.fonts" -iname '*0xProto*' 2>/dev/null | grep -q .; then
        echo "ERROR: getnf did not install 0xProto."
        exit 1
    fi
fi

pip install colorz --break-system-packages

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
            rm -f "$HOME/.local/bin/mangowc-start-nonvidia.sh"
            mv "$HOME/.local/bin/mangowc-start-nvidia.sh" "$HOME/.local/bin/mangowc-start.sh"
            sleep 1
            break 
            ;;
        "NO")
            echo "lucky..."
            rm -f "$HOME/.local/bin/mangowc-start-nvidia.sh"
            mv "$HOME/.local/bin/mangowc-start-nonvidia.sh" "$HOME/.local/bin/mangowc-start.sh"
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

echo "Configuring SDDM, theme, and boot splash..."
sudo bash -c "
set -euo pipefail
systemctl enable sddm
systemctl set-default graphical.target
cp -r \"$THE_STUFF/anomalous\" /usr/share/sddm/themes/
cp \"$THE_STUFF/etc/sddm.conf\" /etc/sddm.conf
chown -R \"$USER:$USER\" /usr/share/sddm/themes/anomalous
if [[ -f \"$THE_STUFF/splash-arch.bmp\" ]]; then
    install -d /usr/local/share/anomale /usr/local/lib/anomale /etc/pacman.d/hooks
    install -m 644 \"$THE_STUFF/splash-arch.bmp\" /usr/local/share/anomale/splash.bmp
    install -m 755 \"$THE_STUFF/usr/local/lib/anomale/restore-splash.sh\" /usr/local/lib/anomale/restore-splash.sh
    install -m 644 \"$THE_STUFF/etc/pacman.d/hooks/anomale-splash.hook\" /etc/pacman.d/hooks/anomale-splash.hook
    /usr/local/lib/anomale/restore-splash.sh --force-rebuild
fi
"

# Theme dir is user-owned so these work without sudo (intentional).
cp "$HOME/.cache/wal/sddm.conf" "/usr/share/sddm/themes/anomalous/theme.conf"
cp "$(< "$HOME/.cache/wal/wal")" "/usr/share/sddm/themes/anomalous/background.jpg"

fish << EOF
if not contains "$HOME/.local/bin" \$fish_user_paths
    set -Ua fish_user_paths "$HOME/.local/bin"
end
set -Ux TERMINAL foot
EOF
clear

mkdir -p "$HOME/.config/systemd/user"
cat << 'EOF' > "$HOME/.config/systemd/user/mangowm-session.target"
[Unit]
Description=MangoWM Session
BindsTo=graphical-session.target
Wants=graphical-session-pre.target
After=graphical-session-pre.target
EOF

mkdir -p "$HOME/.config/xdg-desktop-portal-wlr"
cat << 'EOF' > "$HOME/.config/xdg-desktop-portal-wlr/wlroots"
[screencast]
chooser_cmd=slurp -f %o -or
chooser_type=simple
EOF

cat << "EOF"
This is the end of the script. Please reboot your computer.
EOF
