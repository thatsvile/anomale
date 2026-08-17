#!/usr/bin/env bash
# Install OBS Studio from official Arch repos and build the wlrobs plugin from
# upstream (https://hg.sr.ht/~scoopta/wlrobs). Does not use the AUR.
#
# After install: restart OBS, then Sources -> + -> Wayland output (dmabuf)
# (or Wayland output (scpy) if dmabuf fails).
#
# Usage: install-wlrobs.sh [--force]
#   --force   rebuild and reinstall the plugin even if it already exists

set -euo pipefail

WLROBS_URL='https://hg.sr.ht/~scoopta/wlrobs/archive/tip.tar.gz'

PACMAN_PACKAGES=(
	obs-studio
	meson
	ninja
	wayland
	pkgconf
	gcc
	curl
	tar
)

FORCE=0
for arg in "$@"; do
	case "$arg" in
	--force) FORCE=1 ;;
	-h | --help)
		sed -n '2,12p' "$0"
		exit 0
		;;
	*)
		echo "unknown argument: $arg" >&2
		exit 2
		;;
	esac
done

die() {
	echo "error: $*" >&2
	exit 1
}

if [[ ${EUID} -eq 0 && -z ${SUDO_USER:-} && -z ${DOAS_USER:-} ]]; then
	die "run this as your user; pacman will be invoked with sudo/doas"
fi

if [[ -n ${SUDO_USER:-} && ${SUDO_USER} != root ]]; then
	TARGET_USER=$SUDO_USER
elif [[ -n ${DOAS_USER:-} && ${DOAS_USER} != root ]]; then
	TARGET_USER=$DOAS_USER
else
	TARGET_USER=${USER:-$(id -un)}
fi

TARGET_HOME=$(getent passwd "$TARGET_USER" | cut -d: -f6)
[[ -n $TARGET_HOME ]] || die "could not resolve home for $TARGET_USER"

if [[ -n ${XDG_CONFIG_HOME:-} && ${EUID} -ne 0 ]]; then
	CONFIG_HOME=$XDG_CONFIG_HOME
else
	CONFIG_HOME=$TARGET_HOME/.config
fi

PLUGIN_DIR=$CONFIG_HOME/obs-studio/plugins/wlrobs/bin/64bit
PLUGIN_SO=$PLUGIN_DIR/libwlrobs.so

as_root() {
	if [[ ${EUID} -eq 0 ]]; then
		"$@"
	elif command -v sudo >/dev/null 2>&1; then
		sudo "$@"
	elif command -v doas >/dev/null 2>&1; then
		doas "$@"
	else
		die "pacman needs root; install sudo or doas"
	fi
}

echo "==> installing OBS and build dependencies with pacman"
as_root pacman -S --needed --noconfirm "${PACMAN_PACKAGES[@]}"

command -v pkg-config >/dev/null 2>&1 || die "pkg-config missing after pacman"
pkg-config --exists libobs || die "libobs.pc not found; obs-studio install looks incomplete"
pkg-config --exists wayland-client || die "wayland-client.pc not found"

if [[ -f $PLUGIN_SO && $FORCE -eq 0 ]]; then
	echo "==> wlrobs already installed at $PLUGIN_SO"
	echo "    rerun with --force to rebuild from upstream"
else
	build_dir=$(mktemp -d "${TMPDIR:-/tmp}/wlrobs-build.XXXXXX")
	cleanup() { rm -rf "$build_dir"; }
	trap cleanup EXIT

	echo "==> downloading wlrobs from upstream"
	curl -fsSL "$WLROBS_URL" -o "$build_dir/wlrobs.tar.gz"
	tar -xzf "$build_dir/wlrobs.tar.gz" -C "$build_dir"
	src_dir=
	for dir in "$build_dir"/wlrobs-*; do
		if [[ -f $dir/meson.build ]]; then
			src_dir=$dir
			break
		fi
	done
	[[ -n $src_dir ]] || die "unexpected wlrobs archive layout"

	echo "==> building wlrobs"
	meson setup "$src_dir/build" "$src_dir" --buildtype=release
	ninja -C "$src_dir/build"

	so=$src_dir/build/libwlrobs.so
	[[ -f $so ]] || die "build succeeded but libwlrobs.so was not produced"

	echo "==> installing plugin for $TARGET_USER"
	install -d -m 0755 "$PLUGIN_DIR"
	install -m 0755 "$so" "$PLUGIN_SO"
	if [[ ${EUID} -eq 0 ]]; then
		chown -R "$TARGET_USER:$TARGET_USER" "$CONFIG_HOME/obs-studio/plugins/wlrobs"
	fi
fi

[[ -f $PLUGIN_SO ]] || die "plugin missing after install: $PLUGIN_SO"

echo
echo "wlrobs is ready: $PLUGIN_SO"
echo "Restart OBS, then add source: Wayland output (dmabuf) or Wayland output (scpy)."
if pgrep -u "$TARGET_USER" -x obs >/dev/null 2>&1; then
	echo "OBS is running; restart it before the new source appears."
fi
