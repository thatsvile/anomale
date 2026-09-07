#!/bin/bash
set -Eeuo pipefail

# Debian Sid/Forky installer for Anomale.
# Parallel to install.sh (Arch). Does not modify Arch paths or Arch helper scripts.

clear
SCRIPT_DIR=$( cd -- "$( dirname -- "${BASH_SOURCE[0]}" )" &> /dev/null && pwd )
THE_STUFF="$SCRIPT_DIR/thestuff"
SUDOERS_DROPIN="/etc/sudoers.d/99-anomale-install"
BUILD_ROOT=""
NIRI_REPO="https://github.com/niri-wm/niri.git"
XWS_REPO="https://github.com/Supreeeme/xwayland-satellite.git"
ADW_GTK3_API="https://api.github.com/repos/lassekongo83/adw-gtk3/releases/latest"
POLKIT_ARCH="/usr/lib/polkit-gnome/polkit-gnome-authentication-agent-1"
POLKIT_DEBIAN="/usr/libexec/polkit-mate-authentication-agent-1"
POLKIT_ARCH_PKILL="polkit-gnome-authentication-agent-1"
POLKIT_DEBIAN_PKILL="polkit-mate-authentication-agent-1"

export PATH="$PATH:/usr/local/bin:$HOME/.local/bin:$HOME/.cargo/bin"

report_failure() {
    local exit_code=$?
    [[ "$BASHPID" == "$$" ]] || return 0
    echo "" >&2
    echo "ERROR: debianinstall.sh aborted at line ${1} (exit ${exit_code})." >&2
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
        sed -i "s|/home/jor|${HOME}|g; s|__ANOMALE_HOME__|${HOME}|g" "$f"
    done < <(grep -rlI -e '/home/jor' -e '__ANOMALE_HOME__' "$dest" 2>/dev/null || true)
}

# Arch session scripts call polkit-gnome; Debian uses GTK3 mate-polkit so the
# same adw-gtk3-dark + gtk-css.css theming applies. Only rewrite copies under
# the installing user's home — shipped thestuff/ trees stay Arch-clean.
rewrite_polkit_for_debian() {
    local f
    for f in \
        "$HOME/.local/bin/niri-start-nvidia.sh" \
        "$HOME/.local/bin/niri-start-nonvidia.sh" \
        "$HOME/.local/bin/niri-start.sh" \
        "$HOME/.local/bin/wal-refresh.sh"
    do
        [[ -f "$f" ]] || continue
        sed -i \
            -e "s|${POLKIT_ARCH}|${POLKIT_DEBIAN}|g" \
            -e "s|${POLKIT_ARCH_PKILL}|${POLKIT_DEBIAN_PKILL}|g" \
            "$f"
    done
}

# Run wal against the shipped wallpaper pointer so niri-colors.kdl, sddm.conf,
# gtk-css, etc. exist before first login — same end state as picking a wallpaper.
# Then apply SDDM theme.conf + background.jpg exactly like wal-refresh.sh.
seed_pywal_and_sddm_theme() {
    local wall theme_dir
    theme_dir="/usr/share/sddm/themes/anomalous"
    if [[ ! -f "$HOME/.cache/wal/wal" ]]; then
        echo "ERROR: missing $HOME/.cache/wal/wal after cache copy." >&2
        exit 1
    fi
    wall=$(< "$HOME/.cache/wal/wal")
    if [[ ! -f "$wall" ]]; then
        echo "ERROR: seeded wallpaper missing: $wall" >&2
        exit 1
    fi

    echo "Generating pywal scheme from seeded wallpaper ($wall)..."
    # -n: do not launch a wallpaper setter during install (swaybg comes at session start).
    wal --backend colorz --contrast 2.0 -i "$wall" -n

    if [[ ! -f "$HOME/.cache/wal/niri-colors.kdl" ]]; then
        echo "ERROR: wal did not generate ~/.cache/wal/niri-colors.kdl" >&2
        exit 1
    fi
    if [[ ! -f "$HOME/.cache/wal/sddm.conf" ]]; then
        echo "ERROR: wal did not generate ~/.cache/wal/sddm.conf" >&2
        exit 1
    fi

    if [[ ! -d "$theme_dir" ]]; then
        echo "ERROR: SDDM theme missing at $theme_dir" >&2
        exit 1
    fi

    # Same two copies as ~/.local/bin/wal-refresh.sh
    cp "$HOME/.cache/wal/sddm.conf" "$theme_dir/theme.conf"
    cp "$wall" "$theme_dir/background.jpg"

    if [[ ! -f "$theme_dir/background.jpg" ]]; then
        echo "ERROR: failed to install SDDM background.jpg from seeded wallpaper." >&2
        exit 1
    fi
    if [[ ! -f "$theme_dir/angle-down.png" ]]; then
        # Fallback if theme tree lacked the asset for any reason.
        if [[ -f /usr/share/sddm/themes/maldives/angle-down.png ]]; then
            cp /usr/share/sddm/themes/maldives/angle-down.png "$theme_dir/angle-down.png"
        else
            echo "WARNING: angle-down.png missing from Anomalous theme (session combo arrows)."
        fi
    fi
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

apt_packages_from_list() {
    local list="$1"
    mapfile -t pkgs < <(grep -vE '^\s*(#|$)' "$list")
    if ((${#pkgs[@]} == 0)); then
        echo "ERROR: no packages in $list" >&2
        exit 1
    fi
    echo "Installing apt packages from $(basename "$list")..."
    sudo DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends "${pkgs[@]}"
}

ensure_rust_toolchain() {
    if [[ -f "$HOME/.cargo/env" ]]; then
        # shellcheck disable=SC1091
        source "$HOME/.cargo/env"
    fi
    if ! command -v rustup >/dev/null 2>&1; then
        echo "ERROR: rustup missing after apt install." >&2
        exit 1
    fi
    # Distro rustup may need an explicit default toolchain before cargo works.
    rustup default stable >/dev/null
    if [[ -f "$HOME/.cargo/env" ]]; then
        # shellcheck disable=SC1091
        source "$HOME/.cargo/env"
    fi
    export PATH="$PATH:$HOME/.cargo/bin"
    if ! command -v cargo >/dev/null 2>&1; then
        echo "ERROR: cargo is not on PATH after rustup. Open a new shell or check rustup, then re-run." >&2
        exit 1
    fi
}

clone_or_update_repo() {
    local url="$1" dest="$2"
    if [[ -d "$dest/.git" ]]; then
        echo "Updating existing clone at $dest..."
        git -C "$dest" remote set-url origin "$url"
        git -C "$dest" fetch --depth 1 origin HEAD
        git -C "$dest" checkout -q -B anomale-build FETCH_HEAD
        git -C "$dest" reset --quiet --hard FETCH_HEAD
    else
        rm -rf "$dest"
        git clone --depth 1 "$url" "$dest"
    fi
}

install_niri_from_source() {
    echo "Building niri from upstream..."
    local src
    if [[ -d "$HOME/niri/.git" ]]; then
        src="$HOME/niri"
        clone_or_update_repo "$NIRI_REPO" "$src"
    else
        src="$BUILD_ROOT/niri"
        clone_or_update_repo "$NIRI_REPO" "$src"
    fi

    (
        cd "$src"
        ensure_rust_toolchain
        cargo build --release
    )
    if [[ ! -x "$src/target/release/niri" ]]; then
        echo "ERROR: niri failed to build." >&2
        exit 1
    fi

    sudo install -Dm755 "$src/target/release/niri" /usr/local/bin/niri
    sudo install -Dm755 "$src/resources/niri-session" /usr/local/bin/niri-session
    sudo install -Dm644 "$src/resources/niri.desktop" /usr/share/wayland-sessions/niri.desktop
    sudo install -Dm644 "$src/resources/niri.desktop" /usr/local/share/wayland-sessions/niri.desktop
    sudo install -Dm644 "$src/resources/niri-portals.conf" /usr/local/share/xdg-desktop-portal/niri-portals.conf

    # Manual install layout: unit must point at /usr/local/bin/niri.
    local unit_tmp
    unit_tmp=$(mktemp)
    sed 's|^ExecStart=niri --session$|ExecStart=/usr/local/bin/niri --session|' \
        "$src/resources/niri.service" >"$unit_tmp"
    if ! grep -q '^ExecStart=/usr/local/bin/niri --session$' "$unit_tmp"; then
        echo "ERROR: failed to patch niri.service ExecStart for /usr/local/bin." >&2
        exit 1
    fi
    sudo install -Dm644 "$unit_tmp" /etc/systemd/user/niri.service
    sudo install -Dm644 "$src/resources/niri-shutdown.target" /etc/systemd/user/niri-shutdown.target
    rm -f "$unit_tmp"
    systemctl --user daemon-reload 2>/dev/null || true

    mkdir -p "${XDG_CACHE_HOME:-$HOME/.cache}/anomale/src"
    git -C "$src" rev-parse --short=12 HEAD \
        >"${XDG_CACHE_HOME:-$HOME/.cache}/anomale/src/niri.installed"
}

install_xwayland_satellite_from_source() {
    echo "Building xwayland-satellite from upstream..."
    local src
    if [[ -d "$HOME/xwayland-satellite/.git" ]]; then
        src="$HOME/xwayland-satellite"
        clone_or_update_repo "$XWS_REPO" "$src"
    else
        src="$BUILD_ROOT/xwayland-satellite"
        clone_or_update_repo "$XWS_REPO" "$src"
    fi

    (
        cd "$src"
        ensure_rust_toolchain
        if [[ -f Cargo.lock ]]; then
            cargo build --release --locked
        else
            cargo build --release
        fi
    )
    local bin="$src/target/release/xwayland-satellite"
    if [[ ! -x "$bin" ]]; then
        echo "ERROR: xwayland-satellite failed to build." >&2
        exit 1
    fi
    sudo install -Dm755 "$bin" /usr/local/bin/xwayland-satellite

    mkdir -p "${XDG_CACHE_HOME:-$HOME/.cache}/anomale/src"
    git -C "$src" rev-parse --short=12 HEAD \
        >"${XDG_CACHE_HOME:-$HOME/.cache}/anomale/src/xwayland-satellite.installed"
}

install_adw_gtk3_theme() {
    if [[ -d /usr/share/themes/adw-gtk3-dark ]]; then
        echo "adw-gtk3-dark already present under /usr/share/themes."
        return 0
    fi
    echo "Installing adw-gtk3 theme from upstream release..."
    local tmp asset_url asset_name
    tmp=$(mktemp -d)
    asset_url=$(curl -fsSL "$ADW_GTK3_API" | python3 -c \
        "import sys,json; d=json.load(sys.stdin); print(d['assets'][0]['browser_download_url'])")
    asset_name=$(basename "$asset_url")
    curl -fsSL "$asset_url" -o "$tmp/$asset_name"
    tar -xJf "$tmp/$asset_name" -C "$tmp"
    # Tarball usually contains adw-gtk3/ and adw-gtk3-dark/ at top level or one deep.
    local theme_root
    theme_root=$(find "$tmp" -type d -name 'adw-gtk3-dark' -print -quit)
    if [[ -z "$theme_root" ]]; then
        echo "ERROR: adw-gtk3-dark directory missing from release archive." >&2
        exit 1
    fi
    sudo mkdir -p /usr/share/themes
    sudo cp -a "$(dirname "$theme_root")/adw-gtk3" /usr/share/themes/ 2>/dev/null || true
    sudo cp -a "$theme_root" /usr/share/themes/adw-gtk3-dark
    rm -rf "$tmp"
    if [[ ! -d /usr/share/themes/adw-gtk3-dark ]]; then
        echo "ERROR: adw-gtk3-dark install failed." >&2
        exit 1
    fi
}

install_bluetui() {
    if command -v bluetui >/dev/null 2>&1; then
        echo "bluetui already installed."
        return 0
    fi
    echo "Installing bluetui via cargo..."
    ensure_rust_toolchain
    if ! cargo install bluetui --locked 2>/dev/null; then
        cargo install bluetui
    fi
    if [[ -x "$HOME/.cargo/bin/bluetui" ]]; then
        sudo install -Dm755 "$HOME/.cargo/bin/bluetui" /usr/local/bin/bluetui
    elif command -v bluetui >/dev/null 2>&1; then
        sudo install -Dm755 "$(command -v bluetui)" /usr/local/bin/bluetui
    else
        echo "ERROR: bluetui missing after cargo install." >&2
        exit 1
    fi
}

install_python_packages() {
    echo "Installing Python packages via pip..."

    local stale
    stale=$(pip list --user --format=freeze 2>/dev/null | cut -d= -f1 \
        | grep -x -e pywal16 -e pywalfox -e haishoku -e colorz || true)
    if [[ -n "$stale" ]]; then
        echo "Removing stale user-level Python packages from a previous run..."
        # shellcheck disable=SC2086
        pip uninstall --break-system-packages -y $stale
    fi

    sudo pip install --break-system-packages --upgrade \
        pywal16 \
        pywalfox \
        haishoku \
        colorz

    if ! command -v wal >/dev/null 2>&1 || ! command -v pywalfox >/dev/null 2>&1; then
        echo "ERROR: wal/pywalfox missing from PATH after pip install."
        exit 1
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
    stamp_dir="${XDG_CACHE_HOME:-$HOME/.cache}/anomale/src"
    mkdir -p "$stamp_dir"
    echo "$tag" >"$stamp_dir/wifitui.installed"
    rm -rf "$tmp"
}

setup_pywalfox() {
    echo "Configuring Pywalfox native messaging host..."
    local pywalfox_bin
    pywalfox_bin=$(command -v pywalfox || true)
    if [[ -z "$pywalfox_bin" ]]; then
        echo "ERROR: pywalfox not on PATH after pip install."
        exit 1
    fi

    sudo "$pywalfox_bin" install --global

    "$pywalfox_bin" install --manifest-path "$HOME/.mozilla/native-messaging-hosts" \
        --profile-path "$HOME/.librewolf"
}

#welcome
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
Expanding on the philosophy that niri offers, anomale shell does **not** include 
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
Debian Sid/Forky installation with 
no DE or display manager. (The script may work if used under 
different conditions, but no promises. It installs from official 
Debian packages plus a few trusted upstream sources — niri and
xwayland-satellite are built from git. Use install.sh on Arch.)

After considering all of this, you may proceed.
EOF
sleep 0.3

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

echo "Starting the Debian installation..."
echo "You may be asked for your password once or twice; the script keeps sudo alive afterward."
sleep 2
sudo -v

echo "Defaults:${USER} timestamp_timeout=180" | sudo tee "$SUDOERS_DROPIN" >/dev/null
sudo chmod 440 "$SUDOERS_DROPIN"
sudo visudo -cf "$SUDOERS_DROPIN" >/dev/null

while true; do
    sudo -n true
    sleep 60
    kill -0 "$$" || exit
done 2>/dev/null &

clear
echo "Checking network..."
if ! curl -fsSL --connect-timeout 8 -o /dev/null https://deb.debian.org/; then
    echo "ERROR: No network reachability to deb.debian.org. Configure network and try again."
    exit 1
fi

if [[ -r /etc/os-release ]]; then
    # shellcheck disable=SC1091
    . /etc/os-release
    case "${VERSION_CODENAME:-}${PRETTY_NAME:-}" in
        *sid*|*forky*|*Sid*|*Forky*)
            ;;
        *)
            echo "WARNING: This installer targets Debian Sid/Forky. Detected: ${PRETTY_NAME:-unknown}."
            echo "Continuing anyway..."
            sleep 2
            ;;
    esac
fi

echo "Updating apt package indexes..."
sudo apt-get update

echo "Ensuring git, curl, and build-essential are installed..."
sudo DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
    git curl ca-certificates build-essential

BUILD_ROOT=$(mktemp -d)

apt_packages_from_list "$THE_STUFF/debpackagelist.txt"
ensure_rust_toolchain
install_niri_from_source
install_xwayland_satellite_from_source
install_adw_gtk3_theme
install_python_packages
install_getnf
install_wifitui
install_bluetui
setup_pywalfox

sudo chsh -s /usr/bin/fish "$USER"

# Required to build anomale
ensure_rust_toolchain

echo "Building Anomale..."
(cd "$THE_STUFF/shell/" && cargo build --release)
if [[ ! -x "$THE_STUFF/shell/target/release/anomale" ]]; then
    echo "ERROR: Anomale failed to build (missing target/release/anomale)."
    exit 1
fi
mkdir -p "$HOME/.local/bin/"
cp "$THE_STUFF/shell/target/release/anomale" "$HOME/.local/bin/"
chmod +x "$HOME/.local/bin/anomale"

stamp_dir="${XDG_CACHE_HOME:-$HOME/.cache}/anomale/src"
mkdir -p "$stamp_dir"
if git -C "$SCRIPT_DIR/.." rev-parse --is-inside-work-tree >/dev/null 2>&1; then
    git -C "$SCRIPT_DIR/.." rev-parse --short=12 HEAD:anomale/thestuff/shell \
        >"$stamp_dir/shell.installed" 2>/dev/null || true
fi

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
# Debian updater tracks niri + xwayland-satellite; Arch keeps thestuff/.local/bin/anomale-apps.
if [[ -f "$THE_STUFF/debian/anomale-apps" ]]; then
    cp "$THE_STUFF/debian/anomale-apps" "$HOME/.local/bin/anomale-apps"
else
    echo "ERROR: missing Debian anomale-apps at $THE_STUFF/debian/anomale-apps" >&2
    exit 1
fi
chmod +x "$HOME/.local/bin/"*

rewrite_polkit_for_debian

if [[ ! -f "$HOME/.local/bin/niri-start-nvidia.sh" || ! -f "$HOME/.local/bin/niri-start-nonvidia.sh" ]]; then
    echo "ERROR: niri start scripts missing from ~/.local/bin after copy."
    exit 1
fi
if ! grep -q "$POLKIT_DEBIAN" "$HOME/.local/bin/niri-start-nonvidia.sh"; then
    echo "ERROR: Debian polkit path rewrite failed in niri-start scripts."
    exit 1
fi

fish -c "set -Ux TERMINAL foot"

rm -f "$HOME/.config/gtk-4.0/gtk.css"
rm -f "$HOME/.config/gtk-4.0/gtk-dark.css"
rm -f "$HOME/.config/gtk-3.0/gtk.css"
rm -f "$HOME/.config/gtk-3.0/gtk-dark.css"

ln -s "$HOME/.cache/wal/gtk-css.css" "$HOME/.config/gtk-4.0/gtk.css"
ln -s "$HOME/.cache/wal/gtk-css.css" "$HOME/.config/gtk-4.0/gtk-dark.css"
ln -s "$HOME/.cache/wal/gtk-css.css" "$HOME/.config/gtk-3.0/gtk.css"
ln -s "$HOME/.cache/wal/gtk-css.css" "$HOME/.config/gtk-3.0/gtk-dark.css"

echo "Installing 0xProto Nerd Font..."
getnf -i "0xProto"
if ! getnf -l 2>/dev/null | grep -qi '0xProto'; then
    if ! find "$HOME/.local/share/fonts" "$HOME/.fonts" -iname '*0xProto*' 2>/dev/null | grep -q .; then
        echo "ERROR: getnf did not install 0xProto."
        exit 1
    fi
fi

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
rewrite_polkit_for_debian
clear

echo "Configuring SDDM, theme, splash asset, and LibreWolf policies..."
sudo bash -c "
set -euo pipefail
systemctl enable sddm
systemctl set-default graphical.target
cp -r \"$THE_STUFF/anomalous\" /usr/share/sddm/themes/
cp \"$THE_STUFF/etc/sddm.conf\" /etc/sddm.conf
chown -R \"$USER:$USER\" /usr/share/sddm/themes/anomalous
install -d /usr/local/share/anomale /usr/local/lib/anomale /etc/apt/apt.conf.d
if [[ -f \"$THE_STUFF/splash-arch.bmp\" ]]; then
    install -m 644 \"$THE_STUFF/splash-arch.bmp\" /usr/local/share/anomale/splash.bmp
fi
if [[ -f \"$THE_STUFF/etc/librewolf/policies.json\" ]]; then
    install -m 644 \"$THE_STUFF/etc/librewolf/policies.json\" /usr/local/share/anomale/librewolf-policies.json
    install -m 755 \"$THE_STUFF/usr/local/lib/anomale/librewolf-pywalfox-policy-debian.sh\" /usr/local/lib/anomale/librewolf-pywalfox-policy-debian.sh
    install -m 644 \"$THE_STUFF/etc/apt/apt.conf.d/99anomale-librewolf\" /etc/apt/apt.conf.d/99anomale-librewolf
    /usr/local/lib/anomale/librewolf-pywalfox-policy-debian.sh
fi
"

# Must run after anomalous is installed and ~/.cache/wal paths are rewritten.
seed_pywal_and_sddm_theme

# Re-link GTK CSS after wal regenerated gtk-css.css (symlinks may already be correct).
rm -f "$HOME/.config/gtk-4.0/gtk.css" "$HOME/.config/gtk-4.0/gtk-dark.css"
rm -f "$HOME/.config/gtk-3.0/gtk.css" "$HOME/.config/gtk-3.0/gtk-dark.css"
ln -s "$HOME/.cache/wal/gtk-css.css" "$HOME/.config/gtk-4.0/gtk.css"
ln -s "$HOME/.cache/wal/gtk-css.css" "$HOME/.config/gtk-4.0/gtk-dark.css"
ln -s "$HOME/.cache/wal/gtk-css.css" "$HOME/.config/gtk-3.0/gtk.css"
ln -s "$HOME/.cache/wal/gtk-css.css" "$HOME/.config/gtk-3.0/gtk-dark.css"

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
This is the end of the Debian script. Please reboot your computer.
EOF
