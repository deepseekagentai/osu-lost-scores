#!/bin/sh
set -eu

APP_ID="kz.lemon4ik.LostScores"
HERE="$(cd "$(dirname "$0")" && pwd)"
DATA="${XDG_DATA_HOME:-$HOME/.local/share}"
BIN="${XDG_BIN_HOME:-$HOME/.local/bin}"
if [ -d "$HERE/icons" ]; then
    ICONS="$HERE/icons"
else
    ICONS="$HERE/../../crates/lost-scores-gui/assets/icon"
fi

if [ -f "$HERE/lost-scores-gui" ]; then
    mkdir -p "$BIN"
    for program in lost-scores-gui lost-scores; do
        cp "$HERE/$program" "$BIN/$program.new"
        mv -f "$BIN/$program.new" "$BIN/$program"
    done
    binary="$BIN/lost-scores-gui"
    echo "installed lost-scores-gui and lost-scores into $BIN"
elif binary="$(command -v lost-scores-gui)"; then
    :
else
    binary=""
    echo "warning: lost-scores-gui is not on PATH; the launcher will not start it until it is" >&2
fi

mkdir -p "$DATA/applications"
for size in 16 24 32 48 64 128 256 512; do
    target="$DATA/icons/hicolor/${size}x${size}/apps"
    mkdir -p "$target"
    cp "$ICONS/lemon-$size.png" "$target/$APP_ID.png"
done

desktop="$DATA/applications/$APP_ID.desktop"
cp "$HERE/$APP_ID.desktop" "$desktop"
if [ -n "$binary" ]; then
    sed -i "s|^Exec=.*|Exec=$binary|" "$desktop"
fi

if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database "$DATA/applications" || true
fi
if command -v gtk-update-icon-cache >/dev/null 2>&1; then
    gtk-update-icon-cache -q -t "$DATA/icons/hicolor" || true
fi

echo "installed $desktop"
