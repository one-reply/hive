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
minimum_os="$(xcrun vtool -show-build "$executable" | awk '/minos/ {print $2; exit}')"
[[ -n "$minimum_os" ]] || { echo "Missing Mach-O deployment version." >&2; exit 1; }
output="$(dirname "$executable")/bundle"
mkdir -p "$output"
stage="$(mktemp -d "$output/.hive.XXXXXX")"
trap 'rm -rf "$stage"' EXIT
app="$stage/Hive.app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$executable" "$app/Contents/MacOS/hive"
cp crates/hive/assets/icons/Hive-light.icns "$app/Contents/Resources/Hive.icns"
cp crates/hive/assets/icons/Hive-dark.icns "$app/Contents/Resources/Hive-dark.icns"
xcrun actool crates/hive/assets/icons/Hive.icon \
    --compile "$app/Contents/Resources" --platform macosx \
    --minimum-deployment-target "$minimum_os" --app-icon Hive \
    --output-partial-info-plist "$stage/icon-info.plist" --output-format human-readable-text
python3 - "$app" "$version" "$minimum_os" "$stage/icon-info.plist" <<'PY'
import plistlib, sys
from pathlib import Path
app, version, minimum_os, partial = sys.argv[1:]
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
PY
plutil -lint "$app/Contents/Info.plist"
codesign --force --sign - "$app"
codesign --verify --strict "$app"
rm -rf "$output/Hive.app"
mv "$app" "$output/Hive.app"
echo "Built $output/Hive.app"
