#!/bin/bash

WAL_COLOR=$(jq -r '.colors.color3' ~/.cache/wal/colors.json)

FOLDER_COLOR=$(python3 ~/.local/bin/folder-colors.py "$WAL_COLOR")

# Silence status text so in-process colorlab apply is not corrupted.
papirus-folders -C "$FOLDER_COLOR" --theme Vile-Papirus >/dev/null 2>&1
