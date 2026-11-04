#!/bin/bash
set -euo pipefail

cd "$(dirname "$0")/.."
build_args=(--package hive --message-format=json-render-diagnostics)
case "${1:-debug}" in
    debug) ;;
    release) build_args+=(--release) ;;
    *) echo "Usage: $0 [debug|release]" >&2; exit 2 ;;
esac
[[ $# -le 1 && "$(uname -s)" == Darwin ]] || { echo "Requires macOS; use [debug|release]." >&2; exit 2; }
python3 scripts/check-icons.py
executable="$(cargo build "${build_args[@]}" | python3 -c '
import json, sys
paths = [m["executable"] for line in sys.stdin if (m := json.loads(line)).get("reason") == "compiler-artifact"
         and m["target"]["name"] == "hive" and m.get("executable")]
assert len(paths) == 1, "Cargo did not produce the hive executable"
print(paths[0])
')"
version="$(cargo metadata --no-deps --format-version 1 | python3 -c '
import json, sys
print(next(p["version"] for p in json.load(sys.stdin)["packages"] if p["name"] == "hive"))
')"
# macOS ships Apple Silicon only (wiki/decisions/macos-apple-silicon-only.md): refuse Intel or universal.
architectures="$(lipo -archs "$executable")"
[[ "$architectures" == arm64 ]] || { echo "Expected an arm64-only executable, got: $architectures" >&2; exit 1; }
minimum_os="$(xcrun vtool -show-build "$executable" | awk '/minos/ {print $2; exit}')"
[[ -n "$minimum_os" ]] || { echo "Missing Mach-O deployment version." >&2; exit 1; }
output="$(dirname "$executable")/bundle"
mkdir -p "$output"
stage="$(mktemp -d "$output/.hive.XXXXXX")"
trap 'rm -rf "$stage"' EXIT
app="$stage/Hive.app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$executable" "$app/Contents/MacOS/hive"
xcrun actool crates/hive/assets/icons/Hive.icon \
    --compile "$app/Contents/Resources" --platform macosx \
    --minimum-deployment-target "$minimum_os" --app-icon Hive \
    --output-partial-info-plist "$stage/icon-info.plist" --output-format human-readable-text
# actool's fallback contains fewer sizes; keep our complete 16–1024 px ICNS.
cp crates/hive/assets/icons/Hive-light.icns "$app/Contents/Resources/Hive.icns"
xcrun assetutil --info "$app/Contents/Resources/Assets.car" > "$stage/catalog.json"
python3 - "$app" "$version" "$minimum_os" "$stage/icon-info.plist" "$stage/catalog.json" <<'PY'
import json, plistlib, sys
from pathlib import Path
app, version, minimum_os, partial, catalog = sys.argv[1:]
contents = Path(app) / "Contents"
info = plistlib.loads(Path("packaging/macos/Info.plist").read_bytes())
info.update(plistlib.loads(Path(partial).read_bytes()))
info.update(CFBundleShortVersionString=version, CFBundleVersion=version, LSMinimumSystemVersion=minimum_os)
(contents / "Info.plist").write_bytes(plistlib.dumps(info))
assert (contents / "MacOS" / info["CFBundleExecutable"]).is_file()
assert info["CFBundleIconName"] == "Hive"
assert (contents / "Resources/Assets.car").stat().st_size > 0
icon = info["CFBundleIconFile"]
assert (contents / "Resources" / (icon if icon.endswith(".icns") else icon + ".icns")).is_file()
assert (contents / "Resources/Hive.icns").read_bytes() == Path("crates/hive/assets/icons/Hive-light.icns").read_bytes()
appearances = {item.get("Appearance") for item in json.loads(Path(catalog).read_text())
               if item.get("AssetType") == "IconImageStack" and item.get("Name") == "Hive"}
assert {"NSAppearanceNameAqua", "NSAppearanceNameDarkAqua"} <= appearances
PY
plutil -lint "$app/Contents/Info.plist"
codesign --force --sign - "$app"
codesign --verify --strict "$app"
rm -rf "$output/Hive.app"
mv "$app" "$output/Hive.app"
echo "Built $output/Hive.app"
