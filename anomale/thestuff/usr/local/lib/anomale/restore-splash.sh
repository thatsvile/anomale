#!/bin/bash
# Keep Anomale boot splash wired into mkinitcpio UKIs (systemd-stub .splash section).
# Applies whether GRUB chainloads the UKI or the firmware boots it directly.
set -euo pipefail

SPLASH="/usr/local/share/anomale/splash.bmp"
PRESET_DIR="/etc/mkinitcpio.d"

if [[ ! -f "$SPLASH" ]]; then
    echo "anomale-splash: missing $SPLASH" >&2
    exit 1
fi

shopt -s nullglob
presets=("$PRESET_DIR"/*.preset)
if ((${#presets[@]} == 0)); then
    echo "anomale-splash: no presets in $PRESET_DIR" >&2
    exit 0
fi

# mkinitcpio 39+ reads ALL_splash/<preset>_splash from the preset. Older versions
# only honour --splash inside <preset>_options, so pick whichever this build supports.
use_all_splash=0
if mkinitcpio_bin=$(command -v mkinitcpio) && grep -q '_splash' "$mkinitcpio_bin"; then
    use_all_splash=1
fi

# Escape for sed replacements (delimiter is |).
splash_sed=${SPLASH//\\/\\\\}
splash_sed=${splash_sed//&/\\&}
splash_sed=${splash_sed//|/\\|}

strip_splash_from_options() {
    sed -i -E \
        -e "/_options=/ s|--splash[=[:space:]]+\"[^\"]*\"||g" \
        -e "/_options=/ s|--splash[=[:space:]]+'[^']*'||g" \
        -e "/_options=/ s|--splash[=[:space:]]+[^\"'[:space:]]+||g" \
        -e "/_options=/ s|[[:space:]]{2,}| |g" \
        -e "/_options=/ s|=([\"'])[[:space:]]+|=\1|" \
        -e "/_options=/ s|[[:space:]]+([\"'])[[:space:]]*$|\1|" \
        "$1"
}

insert_after() {
    local preset=$1 regex=$2 newline=$3 tmp
    tmp=$(mktemp)
    awk -v re="$regex" -v ins="$newline" '
        !inserted && $0 ~ re { print; print ins; inserted = 1; next }
        { print }
    ' "$preset" > "$tmp"
    cat "$tmp" > "$preset"
    rm -f "$tmp"
}

set_all_splash() {
    local preset=$1

    # Point any active ALL_splash / <preset>_splash assignments at our image.
    sed -i -E "s|^([[:space:]]*[A-Za-z0-9_]+_splash=).*|\1\"${splash_sed}\"|" "$preset"

    grep -qE '^[[:space:]]*ALL_splash=' "$preset" && return

    # Prefer reusing the stock commented-out line so the preset stays tidy.
    if grep -qE '^[[:space:]]*#[[:space:]]*ALL_splash=' "$preset"; then
        sed -i -E "s|^[[:space:]]*#[[:space:]]*ALL_splash=.*|ALL_splash=\"${splash_sed}\"|" "$preset"
    elif grep -qE '^[[:space:]]*ALL_kver=' "$preset"; then
        insert_after "$preset" '^[[:space:]]*ALL_kver=' "ALL_splash=\"${SPLASH}\""
    elif grep -qE '^[[:space:]]*PRESETS=' "$preset"; then
        insert_after "$preset" '^[[:space:]]*PRESETS=' "ALL_splash=\"${SPLASH}\""
    else
        printf '\nALL_splash="%s"\n' "$SPLASH" >> "$preset"
    fi
}

set_options_splash() {
    local preset=$1 name

    # Legacy mkinitcpio: every UKI preset carries --splash in its own options.
    sed -i -E \
        -e "/_options=/ s|--splash[=[:space:]]+\"[^\"]*\"|--splash=${splash_sed}|g" \
        -e "/_options=/ s|--splash[=[:space:]]+'[^']*'|--splash=${splash_sed}|g" \
        -e "/_options=/ s|--splash[=[:space:]]+[^\"'[:space:]]+|--splash=${splash_sed}|g" \
        "$preset"

    while read -r name; do
        [[ -n "$name" ]] || continue
        if ! grep -qE "^[[:space:]]*${name}_options=" "$preset"; then
            printf '%s_options="--splash=%s"\n' "$name" "$SPLASH" >> "$preset"
        elif ! grep -qE "^[[:space:]]*${name}_options=.*--splash" "$preset"; then
            sed -i -E \
                -e "s|^([[:space:]]*${name}_options=[\"'])|\1--splash=${splash_sed} |" \
                -e "/^[[:space:]]*${name}_options=/ s|[[:space:]]+([\"'])[[:space:]]*$|\1|" \
                "$preset"
        fi
    done < <(sed -nE 's|^[[:space:]]*([A-Za-z0-9_]+)_uki=.*|\1|p' "$preset")
}

changed=0
has_uki=0
for preset in "${presets[@]}"; do
    if grep -qE '^[[:space:]]*[A-Za-z0-9_]+_uki=' "$preset"; then
        has_uki=1
    fi

    before=$(sha256sum "$preset" | awk '{print $1}')
    if ((use_all_splash == 1)); then
        strip_splash_from_options "$preset"
        set_all_splash "$preset"
    else
        set_options_splash "$preset"
    fi
    after=$(sha256sum "$preset" | awk '{print $1}')

    if [[ "$before" != "$after" ]]; then
        echo "anomale-splash: wired splash into $preset"
        changed=1
    fi
done

if ((has_uki == 0)); then
    echo "anomale-splash: warning: no active *_uki= in $PRESET_DIR — splash only applies to UKIs" >&2
fi

if ((changed == 1)) || [[ "${1:-}" == "--force-rebuild" ]]; then
    echo "anomale-splash: rebuilding initramfs/UKI images..."
    mkinitcpio -P
else
    echo "anomale-splash: presets already point at $SPLASH"
fi
