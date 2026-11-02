# vilefox

Themes Firefox/LibreWolf from `$XDG_CACHE_HOME/vile/wal/colors.json` (not `~/.cache/wal`).
The host starts when the addon connects; applying colors through `vilecolors::export_palette`
(vile `--colors`, colorlab apply, daemon Colors, or `vilecolors` itself) is enough.

## Chrome mapping

`~/.config/vile/vilefox.json` maps Firefox Theme API keys to palette slots
(`color0`…`color15`, `background`, `foreground`, `cursor`) or a `#rrggbb` hex.
`dark` and `light` are chosen from `colors.json` `vile.theme`. Missing keys keep
the built-in defaults; the host reloads color maps on save (no browser restart).

```json
{
  "tab_outline_px": 2,
  "corner_radius_px": 0,
  "urlbar_border_px": 1,
  "compact_tabs": false,
  "tab_close": "always",
  "tab_bar_margin_px": 2,
  "dark": {
    "icons": "color4",
    "tab_line": "color1"
  }
}
```

These chrome keys are written into `userChrome.css` (Theme API cannot set them).
Restart the browser after changing them. Color maps still apply live.

| key | meaning |
| --- | --- |
| `tab_outline_px` | selected-tab outline thickness (1–8) |
| `corner_radius_px` | tabs, url bar, panels, buttons (`0` = square) |
| `urlbar_border_px` | URL field border (0–4) |
| `compact_tabs` | shorter tab strip |
| `tab_close` | `"always"` or `"hover"` |
| `tab_bar_margin_px` | extra left/right padding on the tab strip |

`vilefox install` writes the full default map if the file is missing.
`vilefox config` prints that path. A copy of the defaults also lives at
`~/.local/share/vilefox/vilefox.example.json`.

## Install

```
cargo install --path vilefox --root ~/.local
vilefox install
```

or from this workspace: `cargo run -p vilefox --release -- install`

Then once per profile:

1. Open `~/.local/share/vilefox/vilefox-signed.xpi` in Firefox/LibreWolf (this crate also keeps `vilefox-signed.xpi`)
2. `about:config` → `toolkit.legacyUserProfileCustomizations.stylesheets` = `true`
3. Restart the browser so `userChrome.css` loads

Replace `vilefox/vilefox-signed.xpi` when AMO signs a new version, then run `vilefox install` again.
Disable pywalfox if it is still calling `theme.update`.
