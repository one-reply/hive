#!/bin/bash
set -euo pipefail

cd "$(dirname "$0")/.."
icons="$PWD/crates/hive/assets/icons"
developer_dir="$(xcode-select -p)"
ictool="$developer_dir/../Applications/Icon Composer.app/Contents/Executables/ictool"
if [[ ! -x "$ictool" ]]; then
    echo "Icon Composer from Xcode 26 or later is required." >&2
    exit 1
fi
stage="$(mktemp -d)"
trap 'rm -rf "$stage"' EXIT

for appearance in light dark; do
    rendition=Default
    [[ "$appearance" == dark ]] && rendition=Dark
    iconset="$icons/Hive-$appearance.iconset"
    mkdir -p "$iconset"
    "$ictool" "$icons/Hive.icon" --export-image \
        --output-file "$stage/hive-$appearance.png" \
        --platform macOS --rendition "$rendition" --width 1024 --height 1024 --scale 1
    swift -module-cache-path "${TMPDIR:-/tmp}/hive-swift-cache" scripts/inset-macos-icon.swift \
        "$stage/hive-$appearance.png" "$icons/hive-$appearance.png"
    for size in 16 32 128 256 512; do
        for scale in 1 2; do
            suffix=""
            [[ "$scale" == 2 ]] && suffix="@2x"
            pixels=$((size * scale))
            sips --resampleHeightWidth "$pixels" "$pixels" "$icons/hive-$appearance.png" \
                --out "$iconset/icon_${size}x${size}${suffix}.png" >/dev/null
        done
    done
    iconutil --convert icns "$iconset" --output "$icons/Hive-$appearance.icns"
done

python3 scripts/check-icons.py
