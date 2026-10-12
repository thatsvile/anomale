#!/bin/bash

WAL_COLOR=$(jq -r '.colors.color6' ~/.cache/wal/colors.json)

FOLDER_COLOR=$(python3 ~/.local/bin/folder-colors.py "$WAL_COLOR")

papirus-folders -C "$FOLDER_COLOR" --theme Anomale-Papirus
