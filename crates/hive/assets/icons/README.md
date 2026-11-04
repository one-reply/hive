# Hive icons

A modular H: three separate shapes, with the same geometry at every size.
The light variant is black on white; the dark variant is white on black.

| File | Purpose |
| --- | --- |
| `Hive.icon/` | Editable Icon Composer source with light and dark appearances; SVG in `Assets/mark.svg` |
| `hive-light.png`, `hive-dark.png` | Transparent 1024×1024 canvas, with 100 px macOS padding around an 824×824 tile |
| `Hive-light.iconset/`, `Hive-dark.iconset/` | Ten PNGs each: 16, 32, 128, 256, 512 pt at 1× and 2×, up to 1024 physical pixels |
| `Hive-light.icns`, `Hive-dark.icns` | Complete static macOS icon sets |
| `source/generated-mark.png` | Generated sketch before normalizing the geometry into SVG; not an app resource |

In the UI, `Icon::data` uses the same SVG as a mask with the current theme color.
The logo appears in the title bar and welcome screen.

## Rebuilding

From the repository root, on macOS with Xcode 26+ (Icon Composer), Swift and Python 3:

```sh
bash scripts/generate-icons.sh
python3 scripts/check-icons.py
python3 scripts/install-release-tools.py target/release-tools
export PATH="$PWD/target/release-tools:$PATH"
bash scripts/bundle-macos.sh          # target/debug/bundle/Hive.app
bash scripts/bundle-macos.sh release  # target/release/bundle/Hive.app
open target/debug/bundle/Hive.app
```

`generate-icons.sh` exports both appearances with Apple's `ictool`, adds macOS padding
through CoreGraphics, resizes PNGs with `sips`, and builds `.icns` with `iconutil`.
The checks verify PNG dimensions/alpha and small and Retina images in each `.icns`.

Packaging compiles `.icon` into `Assets.car` with `actool`, validates both appearances,
writes the Cargo version and Mach-O deployment target into `Info.plist`, includes the
static `.icns`, and applies an ad-hoc signature. The icon is available in Finder, the
Dock and the app switcher. Builds are ARM64 only, without Developer ID or notarization.

macOS 26+ chooses the native icon appearance using the system icon style. Hive's
theme setting controls its interface. A static light fallback is included for older
macOS versions; this is not a claim that those systems have been tested.
`cargo run` launches without `.app` metadata; use the bundled `Hive.app` to check
Finder and Dock behavior. See the [release process](../../../../docs/RELEASING.md).

## Sources

- [Apple: Icon Composer](https://developer.apple.com/documentation/xcode/creating-your-app-icon-using-icon-composer)
- [Apple: iconset contents](https://developer.apple.com/library/archive/documentation/Xcode/Reference/xcode_ref-Asset_Catalog_Format/IconSetType.html)
- Generated sketch: the built-in `imagegen` tool, with the [original prompt](source/prompt.md).
