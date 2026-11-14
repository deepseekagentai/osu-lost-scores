# Tools

## prep_share_assets.py

Regenerates `assets/share/` (grades, mod icons, backdrops, lemon and small icons) from the SVGs in `assets/`.

Requirements, with the versions the committed PNGs were made with:

- the `resvg` command line tool, 0.49.0
- Python 3 with Pillow, 12.2

Regenerate from the repository root:

```
python3 -I crates/lost-scores-gui/tools/prep_share_assets.py
```

The script works in a temporary folder that is removed when it ends, also when `resvg` fails. With the versions above the PNGs come out byte-identical, so `git status` should show no change.

## make_icons.py

Regenerates the window icons in `assets/icon/` from `assets/lemon.svg`: the PNGs from 16 to 1024 px, `lemon.ico` for Windows and `lemon.icns` for macOS. Every size is drawn by `resvg` at exactly that size; nothing is scaled. With `resvg` 0.49.0 the files come out byte-identical.

```
cd crates/lost-scores-gui && python3 -I tools/make_icons.py assets/lemon.svg assets/icon
```
