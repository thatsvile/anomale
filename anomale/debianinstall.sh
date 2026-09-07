#!/bin/bash
set -Eeuo pipefail

# Debian Forky/Sid installer for Anomale.
# On Trixie, offers to migrate apt sources to Forky then re-run after upgrade/reboot.
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

debian_codename() {
    if [[ -r /etc/os-release ]]; then
        # shellcheck disable=SC1091
        . /etc/os-release
        echo "${VERSION_CODENAME:-}"
    fi
}

apt_candidate() {
    local pkg="$1" cand
    cand=$(apt-cache policy "$pkg" 2>/dev/null | awk '/Candidate:/ {print $2; exit}')
    if [[ -z "$cand" || "$cand" == "(none)" ]]; then
        return 1
    fi
    return 0
}

resolve_wlroots_package() {
    # Forky/Sid ship libwlroots-0.20; keep 0.18 as a last-resort fallback.
    if apt_candidate libwlroots-0.20; then
        echo "libwlroots-0.20"
    elif apt_candidate libwlroots-0.18; then
        echo "libwlroots-0.18"
    else
        echo "ERROR: neither libwlroots-0.20 nor libwlroots-0.18 is available." >&2
        exit 1
    fi
}

apt_packages_from_list() {
    local list="$1"
    local -a pkgs=()
    local line resolved
    while IFS= read -r line; do
        [[ "$line" =~ ^[[:space:]]*(#|$) ]] && continue
        if [[ "$line" == "libwlroots-0.18" || "$line" == "libwlroots-0.20" ]]; then
            continue
        fi
        pkgs+=("$line")
    done < "$list"

    resolved=$(resolve_wlroots_package)
    pkgs+=("$resolved")
    echo "Using wlroots package: $resolved"

    if ((${#pkgs[@]} == 0)); then
        echo "ERROR: no packages in $list" >&2
        exit 1
    fi
    echo "Installing apt packages from $(basename "$list")..."
    if ! sudo DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends "${pkgs[@]}"; then
        echo "ERROR: apt-get install failed. Check for packages with no installation candidate:" >&2
        local p
        for p in "${pkgs[@]}"; do
            if ! apt_candidate "$p"; then
                echo "  - missing: $p" >&2
            fi
        done
        exit 1
    fi
}

rewrite_apt_sources_trixie_to_forky() {
    local f
    echo "Rewriting apt sources: trixie → forky..."
    # Drop backports entries (Forky has no trixie-backports equivalent we want).
    if [[ -f /etc/apt/sources.list.d/debian-backports.sources ]]; then
        sudo rm -f /etc/apt/sources.list.d/debian-backports.sources
    fi
    for f in /etc/apt/sources.list /etc/apt/sources.list.d/*; do
        [[ -e "$f" ]] || continue
        [[ "$(basename "$f")" == cuda-* ]] && continue
        [[ "$(basename "$f")" == extrepo_* ]] && continue
        # Remove lines that only exist for stable backports.
        sudo sed -i \
            -e '/trixie-backports/d' \
            -e '/Suites:.*trixie-backports/d' \
            "$f" 2>/dev/null || true
        sudo sed -i \
            -e 's/\btrixie-security\b/forky-security/g' \
            -e 's/\btrixie-updates\b/forky-updates/g' \
            -e 's/\btrixie\b/forky/g' \
            "$f"
    done
}

# Dist-upgrade Trixie → Forky. Always exits after upgrade so the user reboots
# and re-runs this script on a consistent Forky system.
migrate_trixie_to_forky() {
    local opt
    echo ""
    cat <<'EOF'
This installer targets Debian Forky (or Sid).
You are on Debian Trixie (stable). Anomale needs newer packages
(gtk4-layer-shell, wlroots, etc.) that Trixie does not ship cleanly.

The script can rewrite your apt sources to Forky and run a full upgrade.
This is a real dist-upgrade — expect downtime and a reboot afterward.
EOF
    PS3="Upgrade this system from Trixie to Forky?: "
    select opt in "YES — migrate to Forky" "NO — abort install"; do
        case $opt in
            "YES — migrate to Forky")
                break
                ;;
            "NO — abort install")
                echo "Aborted. Re-run on Forky/Sid, or choose YES to migrate."
                exit 1
                ;;
            *)
                echo "Invalid entry. Please pick 1 or 2."
                ;;
        esac
    done

    rewrite_apt_sources_trixie_to_forky
    echo "Updating apt indexes for Forky..."
    sudo apt-get update
    echo "Running apt full-upgrade to Forky (this takes a while)..."
    sudo DEBIAN_FRONTEND=noninteractive apt-get -y full-upgrade
    sudo DEBIAN_FRONTEND=noninteractive apt-get -y autoremove --purge || true

    echo ""
    cat <<'EOF'
Trixie → Forky upgrade finished.

Reboot now, then re-run debianinstall.sh so the rest of Anomale
installs against Forky packages (and a Forky-running system).

  sudo reboot
  # after login:
  bash anomale/anomale/debianinstall.sh
EOF
    exit 0
}

ensure_debian_forky_or_sid() {
    local codename
    codename=$(debian_codename)
    case "$codename" in
        forky|sid)
            echo "Detected Debian ${codename} — OK."
            ;;
        trixie)
            migrate_trixie_to_forky
            ;;
        *)
            echo "ERROR: This installer targets Debian Forky or Sid (got: ${codename:-unknown})." >&2
            echo "Start from a Forky/Sid minimal install, or run on Trixie to migrate to Forky." >&2
            exit 1
            ;;
    esac
}

enable_librewolf_repo() {
    echo "Enabling LibreWolf apt repository via extrepo..."
    sudo DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends extrepo
    # Optional catalog package on Trixie; ignore if unavailable.
    sudo DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends extrepo-offline-data 2>/dev/null || true

    if [[ -f /etc/apt/sources.list.d/extrepo_librewolf.sources ]]; then
        echo "LibreWolf extrepo source already enabled."
    else
        sudo extrepo enable librewolf
    fi
    sudo apt-get update
    if ! apt_candidate librewolf; then
        echo "ERROR: librewolf still has no apt candidate after enabling extrepo." >&2
        exit 1
    fi
}

# GitHub CLI is not in Forky; use the official upstream apt repo.
install_github_cli() {
    local keyring="/etc/apt/keyrings/githubcli-archive-keyring.gpg"
    local list="/etc/apt/sources.list.d/github-cli.list"
    local arch
    arch=$(dpkg --print-architecture)

    echo "Installing GitHub CLI (gh) from cli.github.com apt repo..."
    sudo mkdir -p -m 755 /etc/apt/keyrings /etc/apt/sources.list.d
    curl -fsSL -o /tmp/githubcli-archive-keyring.gpg \
        https://cli.github.com/packages/githubcli-archive-keyring.gpg
    sudo install -m 644 /tmp/githubcli-archive-keyring.gpg "$keyring"
    rm -f /tmp/githubcli-archive-keyring.gpg
    sudo chmod go+r "$keyring"

    echo "deb [arch=${arch} signed-by=${keyring}] https://cli.github.com/packages stable main" \
        | sudo tee "$list" >/dev/null

    sudo apt-get update
    sudo DEBIAN_FRONTEND=noninteractive apt-get install -y gh
    if ! command -v gh >/dev/null 2>&1; then
        echo "ERROR: gh missing from PATH after install." >&2
        exit 1
    fi
    echo "GitHub CLI installed: $(gh --version | head -1)"
}

install_steam() {
    echo "Installing Steam (steam-installer)..."
    ensure_debian_nonfree_components
    sudo dpkg --add-architecture i386
    sudo apt-get update

    if ! apt_candidate steam-installer; then
        echo "ERROR: steam-installer has no apt candidate (need contrib/non-free + i386)." >&2
        exit 1
    fi

    sudo DEBIAN_FRONTEND=noninteractive apt-get install -y steam-installer steam-devices
    if [[ ! -x /usr/games/steam ]]; then
        echo "ERROR: /usr/games/steam missing after steam-installer." >&2
        exit 1
    fi
}

# 32-bit NVIDIA userspace for Proton / Steam (after drivers + CUDA repo are present).
install_steam_nvidia_libs() {
    local -a wanted=(
        nvidia-driver-libs:i386
        libglx-nvidia0:i386
        nvidia-vulkan-icd:i386
        libegl-nvidia0:i386
        libgles-nvidia1:i386
        libgles-nvidia2:i386
        libnvidia-glvkspirv:i386
    )
    local -a pkgs=()
    local p

    echo "Installing NVIDIA :i386 libraries for Steam/Proton..."
    sudo dpkg --add-architecture i386
    sudo apt-get update

    for p in "${wanted[@]}"; do
        if apt_candidate "$p"; then
            pkgs+=("$p")
        else
            echo "  skip (no candidate): $p"
        fi
    done

    if ((${#pkgs[@]} == 0)); then
        echo "ERROR: no NVIDIA :i386 packages available for Steam; check CUDA/non-free repos." >&2
        exit 1
    fi

    echo "Installing: ${pkgs[*]}"
    sudo DEBIAN_FRONTEND=noninteractive apt-get install -y "${pkgs[@]}"
}

install_steam_wrapper_and_desktop() {
    local wrapper_src="$THE_STUFF/debian/steam-fixed"
    local wrapper_dst="$HOME/.local/bin/steam-fixed"
    local desktop_dst="$HOME/.local/share/applications/steam.desktop"
    local valve_desktop="$HOME/.steam/debian-installation/deb-installer/steam.desktop"

    if [[ ! -f "$wrapper_src" ]]; then
        echo "ERROR: missing Debian steam wrapper at $wrapper_src" >&2
        exit 1
    fi

    mkdir -p "$HOME/.local/bin" "$HOME/.local/share/applications"
    install -Dm755 "$wrapper_src" "$wrapper_dst"
    ln -sfr "$wrapper_dst" "$HOME/.local/bin/steam"

    if [[ -f "$valve_desktop" ]]; then
        # Drop optional shebang; point every Exec at our wrapper.
        sed -e '1{/^#!/d;}' \
            -e "s|/usr/games/steam|${wrapper_dst}|g" \
            -e 's/^PrefersNonDefaultGPU=true/# PrefersNonDefaultGPU=true/' \
            -e 's/^X-KDE-RunOnDiscreteGpu=true/# X-KDE-RunOnDiscreteGpu=true/' \
            "$valve_desktop" >"$desktop_dst"
    else
        cat >"$desktop_dst" <<EOF
[Desktop Entry]
Name=Steam
Comment=Application for managing and playing games on Steam
Exec=${wrapper_dst} %U
Icon=steam
Terminal=false
Type=Application
Categories=Network;FileTransfer;Game;
MimeType=x-scheme-handler/steam;x-scheme-handler/steamlink;
Keywords=Games
EOF
    fi

    update-desktop-database "$HOME/.local/share/applications" 2>/dev/null || true
    xdg-mime default steam.desktop x-scheme-handler/steam 2>/dev/null || true
    xdg-mime default steam.desktop x-scheme-handler/steamlink 2>/dev/null || true
    echo "Steam wrapper installed at $wrapper_dst (PATH symlink: ~/.local/bin/steam)"
}

# Ensure base Debian sources expose firmware/driver components.
ensure_debian_nonfree_components() {
    local f changed=0
    echo "Ensuring contrib / non-free / non-free-firmware apt components..."
    for f in /etc/apt/sources.list /etc/apt/sources.list.d/*.list /etc/apt/sources.list.d/*.sources; do
        [[ -e "$f" ]] || continue
        [[ "$(basename "$f")" == cuda-* ]] && continue
        if grep -qE '^[[:space:]]*Components:' "$f" 2>/dev/null; then
            if grep -qE '^[[:space:]]*Components:.*\bmain\b' "$f" \
                && ! grep -qE '^[[:space:]]*Components:.*\bnon-free-firmware\b' "$f"; then
                sudo sed -i -E \
                    's/^([[:space:]]*Components:.*\bmain\b)/\1 contrib non-free non-free-firmware/' \
                    "$f"
                changed=1
            fi
        elif grep -qE '^[[:space:]]*deb(-src)?[[:space:]]' "$f" 2>/dev/null; then
            if grep -qE '^[[:space:]]*deb(-src)?[[:space:]].*\bmain\b' "$f" \
                && ! grep -qE '^[[:space:]]*deb(-src)?[[:space:]].*\bnon-free-firmware\b' "$f"; then
                sudo sed -i -E \
                    's/^([[:space:]]*deb(-src)?[[:space:]].*\bmain\b)([[:space:]]|$)/\1 contrib non-free non-free-firmware\3/' \
                    "$f"
                changed=1
            fi
        fi
    done
    if ((changed)); then
        sudo apt-get update
    fi
}

# Install NVIDIA cuda-keyring for debian12 (580) or debian13 (latest/open).
# debian12 on Trixie needs allow-insecure + gpgv (SHA1 / sqv rejection).
enable_nvidia_cuda_repo() {
    local distro="$1"
    local arch="x86_64"
    local url deb list
    case "$distro" in
        debian12|debian13) ;;
        *)
            echo "ERROR: unsupported NVIDIA CUDA distro label: $distro" >&2
            exit 1
            ;;
    esac

    url="https://developer.download.nvidia.com/compute/cuda/repos/${distro}/${arch}/cuda-keyring_1.1-1_all.deb"
    deb=$(mktemp --suffix=-cuda-keyring.deb)
    echo "Adding NVIDIA CUDA apt repo (${distro}/${arch})..."
    curl -fsSL -o "$deb" "$url"
    sudo dpkg -i "$deb"
    rm -f "$deb"

    if [[ "$distro" == "debian12" ]]; then
        list="/etc/apt/sources.list.d/cuda-debian12-x86_64.list"
        if [[ -f "$list" ]]; then
            echo "Applying debian12 CUDA signing workaround for Trixie (allow-insecure + gpgv)..."
            sudo sed -i \
                's|\[signed-by=/usr/share/keyrings/cuda-archive-keyring.gpg\]|[signed-by=/usr/share/keyrings/cuda-archive-keyring.gpg allow-insecure=yes]|' \
                "$list"
        fi
        sudo DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends gnupg >/dev/null
        echo 'APT::Key::GPGVCommand "/usr/bin/gpgv";' | sudo tee /etc/apt/apt.conf.d/99anomale-nvidia-gpgv >/dev/null
    fi

    sudo apt-get update
}

ensure_linux_headers_for_dkms() {
    local running_hdrs="linux-headers-$(uname -r)"
    if apt_candidate "$running_hdrs"; then
        echo "Installing $running_hdrs for DKMS..."
        sudo DEBIAN_FRONTEND=noninteractive apt-get install -y "$running_hdrs"
        return 0
    fi
    case "$(uname -m)" in
        x86_64)
            echo "Installing linux-headers-amd64 for DKMS..."
            sudo DEBIAN_FRONTEND=noninteractive apt-get install -y linux-headers-amd64
            ;;
        *)
            echo "WARNING: no matching linux-headers candidate for $(uname -r); DKMS may fail."
            ;;
    esac
}

blacklist_nouveau() {
    echo "Blacklisting nouveau..."
    sudo tee /etc/modprobe.d/blacklist-nouveau.conf >/dev/null <<'EOF'
blacklist nouveau
options nouveau modeset=0
EOF
    if command -v update-initramfs >/dev/null 2>&1; then
        sudo update-initramfs -u
    fi
}

# Pascal / GTX 10xx: debian12 CUDA repo, pin 580, proprietary modules.
install_nvidia_drivers_pascal() {
    ensure_debian_nonfree_components
    enable_nvidia_cuda_repo debian12
    ensure_linux_headers_for_dkms

    if ! apt_candidate nvidia-driver-pinning-580; then
        echo "ERROR: nvidia-driver-pinning-580 not available after enabling debian12 CUDA repo." >&2
        exit 1
    fi

    echo "Installing NVIDIA 580 (proprietary) for Pascal / GTX 10xx..."
    sudo DEBIAN_FRONTEND=noninteractive apt-get install -y nvidia-driver-pinning-580
    sudo apt-get update
    sudo DEBIAN_FRONTEND=noninteractive apt-get install -y \
        nvidia-driver \
        nvidia-kernel-dkms \
        nvidia-settings \
        firmware-misc-nonfree

    blacklist_nouveau
    echo "NVIDIA 580 proprietary drivers installed. Reboot required; then check nvidia-smi."
}

# Turing+: debian13 CUDA repo, nvidia-open, no branch pin (apt may upgrade later).
install_nvidia_drivers_turing() {
    ensure_debian_nonfree_components
    enable_nvidia_cuda_repo debian13
    ensure_linux_headers_for_dkms

    if ! apt_candidate nvidia-open; then
        echo "ERROR: nvidia-open not available after enabling debian13 CUDA repo." >&2
        exit 1
    fi

    echo "Installing NVIDIA open drivers (newest available from debian13 CUDA repo)..."
    sudo DEBIAN_FRONTEND=noninteractive apt-get install -y nvidia-open firmware-misc-nonfree

    blacklist_nouveau
    echo "NVIDIA open drivers installed. Reboot required; then check nvidia-smi."
}

prompt_and_install_nvidia_drivers() {
    local gen_opt
    if [[ "$(uname -m)" != "x86_64" ]]; then
        echo "ERROR: automatic NVIDIA CUDA repo install supports x86_64 only (got $(uname -m))." >&2
        exit 1
    fi

    echo ""
    cat <<'EOF'
What NVIDIA GPU generation do you have?
  1) GTX 10xx / Pascal (installs 580 proprietary from NVIDIA debian12 repo; pinned)
  2) RTX 20xx / 30xx / 40xx / 50xx (Turing and newer; newest open drivers from debian13, unpinned)
EOF

    PS3="GPU generation?: "
    select gen_opt in "GTX 10xx / Pascal (580)" "RTX 20xx+ (latest open)"; do
        case $gen_opt in
            "GTX 10xx / Pascal (580)")
                install_nvidia_drivers_pascal
                break
                ;;
            "RTX 20xx+ (latest open)")
                install_nvidia_drivers_turing
                break
                ;;
            *)
                echo "Invalid entry. Please pick 1 or 2."
                ;;
        esac
    done
}

# Distro gtk4-layer-shell below 1.1.0 (e.g. old Trixie) SIGSEGVs Anomale under niri.
# Forky/Sid usually ship 1.3.x — this is a no-op then. Build upstream into /usr/local when needed.
gtk4_layer_shell_version() {
    pkg-config --modversion gtk4-layer-shell-0 2>/dev/null || echo "0"
}

version_lt() {
    # Return 0 if $1 < $2 (dpkg version compare).
    dpkg --compare-versions "$1" lt "$2"
}

install_gtk4_layer_shell_if_needed() {
    local have need="1.1.0" src
    have=$(gtk4_layer_shell_version)
    if ! version_lt "$have" "$need"; then
        echo "gtk4-layer-shell ${have} is new enough (>= ${need})."
        return 0
    fi

    echo "System gtk4-layer-shell is ${have} (need >= ${need}). Building upstream into /usr/local..."
    sudo DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
        meson ninja-build libwayland-dev wayland-protocols libgtk-4-dev \
        gobject-introspection libgirepository1.0-dev pkg-config

    src="$BUILD_ROOT/gtk4-layer-shell"
    clone_or_update_repo "https://github.com/wmww/gtk4-layer-shell.git" "$src"
    (
        cd "$src"
        # -Dvapi=false: Anomale only needs the C library; avoids requiring vapigen/valac.
        meson setup \
            --prefix=/usr/local \
            -Dexamples=false \
            -Ddocs=false \
            -Dtests=false \
            -Dsmoke-tests=false \
            -Dvapi=false \
            build
        ninja -C build
        sudo ninja -C build install
    )
    sudo ldconfig

    export PKG_CONFIG_PATH="/usr/local/lib/pkgconfig:/usr/local/lib/x86_64-linux-gnu/pkgconfig:/usr/local/lib/aarch64-linux-gnu/pkgconfig${PKG_CONFIG_PATH:+:$PKG_CONFIG_PATH}"
    export LD_LIBRARY_PATH="/usr/local/lib:/usr/local/lib/x86_64-linux-gnu:/usr/local/lib/aarch64-linux-gnu${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"

    have=$(gtk4_layer_shell_version)
    if version_lt "$have" "$need"; then
        echo "ERROR: gtk4-layer-shell still reports ${have} after /usr/local install." >&2
        echo "Check PKG_CONFIG_PATH picks up /usr/local (got: ${PKG_CONFIG_PATH:-empty})." >&2
        exit 1
    fi
    echo "gtk4-layer-shell ${have} installed under /usr/local."
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
Debian Forky or Sid installation with 
no DE or display manager. If you are still on Trixie,
the script can migrate apt to Forky (dist-upgrade) first.
It installs from official Debian packages plus
LibreWolf via extrepo and a few upstream builds
(niri, xwayland-satellite). Use install.sh on Arch.

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
    echo "Detected: ${PRETTY_NAME:-Debian} (${VERSION_CODENAME:-unknown})"
fi

echo "Updating apt package indexes..."
sudo apt-get update

echo "Ensuring git, curl, and build-essential are installed..."
sudo DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
    git curl ca-certificates build-essential

ensure_debian_forky_or_sid
enable_librewolf_repo
ensure_debian_nonfree_components
install_github_cli

BUILD_ROOT=$(mktemp -d)

apt_packages_from_list "$THE_STUFF/debpackagelist.txt"
install_steam
ensure_rust_toolchain
install_niri_from_source
install_xwayland_satellite_from_source
install_gtk4_layer_shell_if_needed
install_adw_gtk3_theme
install_python_packages
install_getnf
install_wifitui
install_bluetui
setup_pywalfox

sudo chsh -s /usr/bin/fish "$USER"

# Required to build anomale (prefer /usr/local gtk4-layer-shell when present)
ensure_rust_toolchain
export PKG_CONFIG_PATH="/usr/local/lib/pkgconfig:/usr/local/lib/x86_64-linux-gnu/pkgconfig:/usr/local/lib/aarch64-linux-gnu/pkgconfig${PKG_CONFIG_PATH:+:$PKG_CONFIG_PATH}"
export LD_LIBRARY_PATH="/usr/local/lib:/usr/local/lib/x86_64-linux-gnu:/usr/local/lib/aarch64-linux-gnu${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"

echo "Building Anomale..."
(cd "$THE_STUFF/shell/" && cargo clean 2>/dev/null || true && cargo build --release)
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
install_steam_wrapper_and_desktop
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
NVIDIA_GPU=0

select opt in "${options[@]}"
do
    case $opt in
        "YES")
            echo "sorry..."
            rm -f "$HOME/.local/bin/niri-start-nonvidia.sh"
            mv "$HOME/.local/bin/niri-start-nvidia.sh" "$HOME/.local/bin/niri-start.sh"
            NVIDIA_GPU=1
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

if ((NVIDIA_GPU)); then
    prompt_and_install_nvidia_drivers
    install_steam_nvidia_libs
elif command -v nvidia-smi >/dev/null 2>&1; then
    echo "nvidia-smi present; ensuring Steam NVIDIA :i386 libraries..."
    install_steam_nvidia_libs
fi
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
If you installed NVIDIA drivers, confirm with nvidia-smi after reboot.
EOF
