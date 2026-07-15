#!/bin/bash
# Merge Anomale LibreWolf policies into the package policies.json and /etc.
set -euo pipefail

FRAGMENT="${1:-/usr/local/share/anomale/librewolf-policies.json}"
ETC_POLICIES="/etc/librewolf/policies.json"
DIST_POLICIES="/usr/lib/librewolf/distribution/policies.json"

if [[ ! -f "$FRAGMENT" ]]; then
    echo "anomale-librewolf-policy: missing fragment $FRAGMENT" >&2
    exit 1
fi

install -d /etc/librewolf
install -m 644 "$FRAGMENT" "$ETC_POLICIES"

if [[ ! -f "$DIST_POLICIES" ]]; then
    echo "anomale-librewolf-policy: LibreWolf distribution policies not found yet; /etc policies installed."
    exit 0
fi

python3 - "$FRAGMENT" "$DIST_POLICIES" <<'PY'
import json
import sys
from pathlib import Path

fragment_path = Path(sys.argv[1])
dist_path = Path(sys.argv[2])

fragment = json.loads(fragment_path.read_text())
dist = json.loads(dist_path.read_text())

dist.setdefault("policies", {})
frag_policies = fragment.get("policies", {})

# Deep-merge ExtensionSettings so package defaults (uBlock, blocked search) stay intact.
if "ExtensionSettings" in frag_policies:
    dist["policies"].setdefault("ExtensionSettings", {})
    dist["policies"]["ExtensionSettings"].update(frag_policies["ExtensionSettings"])

for key, value in frag_policies.items():
    if key == "ExtensionSettings":
        continue
    dist["policies"][key] = value

dist_path.write_text(json.dumps(dist, indent=4) + "\n")
print(f"anomale-librewolf-policy: merged into {dist_path}")
PY
