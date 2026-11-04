#!/bin/bash
set -euo pipefail

cd "$(dirname "$0")/.."
build_args=(--locked --package hive --message-format=json-render-diagnostics)
case "${1:-debug}" in
    debug) ;;
    release) build_args+=(--release) ;;
    *) echo "Usage: $0 [debug|release]" >&2; exit 2 ;;
esac
[[ $# -le 1 && "$(uname -s)" == Darwin ]] || { echo "Requires macOS; use [debug|release]." >&2; exit 2; }
python3 scripts/check-icons.py
sparkle="$PWD/target/sparkle"
[[ -f "$sparkle/Sparkle.framework/Sparkle" ]] || {
    echo 'Run python3 scripts/install-sparkle.py target/sparkle first.' >&2; exit 1;
}
executable="$(cargo build "${build_args[@]}" | python3 -c '
import json, sys
paths = [m["executable"] for line in sys.stdin if (m := json.loads(line)).get("reason") == "compiler-artifact"
         and m["target"]["name"] == "hive" and m.get("executable")]
assert len(paths) == 1, "Cargo did not produce the hive executable"
print(paths[0])
')"
version="$(cargo metadata --locked --no-deps --format-version 1 | python3 -c '
import json, sys
print(next(p["version"] for p in json.load(sys.stdin)["packages"] if p["name"] == "hive"))
')"
# Hive ships Apple Silicon only: refuse Intel or universal bundles.
architectures="$(lipo -archs "$executable")"
[[ "$architectures" == arm64 ]] || { echo "Expected an arm64-only executable, got: $architectures" >&2; exit 1; }
minimum_os="$(xcrun vtool -show-build "$executable" | awk '/minos/ {print $2; exit}')"
[[ -n "$minimum_os" ]] || { echo "Missing Mach-O deployment version." >&2; exit 1; }
# The embedded updater can require a newer system than the Rust executable.
minimum_os="$(python3 - "$minimum_os" "$sparkle/Sparkle.framework/Resources/Info.plist" <<'PY'
import plistlib, sys
from pathlib import Path
framework_minimum = plistlib.loads(Path(sys.argv[2]).read_bytes())["LSMinimumSystemVersion"]
print(max(sys.argv[1], framework_minimum, key=lambda version: tuple(map(int, version.split(".")))))
PY
)"
output="$(dirname "$executable")/bundle"
mkdir -p "$output"
stage="$(mktemp -d "$output/.hive.XXXXXX")"
trap 'rm -rf "$stage"' EXIT
app="$stage/Hive.app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
mkdir -p "$app/Contents/Frameworks"
ditto "$sparkle/Sparkle.framework" "$app/Contents/Frameworks/Sparkle.framework"
cp "$executable" "$app/Contents/MacOS/hive"
cp LICENSE NOTICE THIRD_PARTY_NOTICES.md "$app/Contents/Resources/"
cp -R licenses "$app/Contents/Resources/licenses"
cp "$sparkle/LICENSE" "$app/Contents/Resources/licenses/Sparkle.txt"
cargo about generate --locked --fail packaging/third-party.hbs \
    --output-file "$app/Contents/Resources/THIRD_PARTY_LICENSES.html"
# MPL-covered source is shipped alongside the binary instead of relying on an external host.
cargo metadata --locked --format-version 1 --filter-platform aarch64-apple-darwin > "$stage/dependencies.json"
python3 - "$stage/dependencies.json" "$app/Contents/Resources/licenses" <<'PY'
import hashlib, json, sys, tarfile
from pathlib import Path
metadata, destination = sys.argv[1:]
packages = json.loads(Path(metadata).read_text())["packages"]
versions = {(package["name"], package["version"]) for package in packages}
for source in json.loads(Path("licenses/upstream/sources.json").read_text()):
    assert (source["name"], source["version"]) in versions, "Refresh supplemental notices after dependency updates"
    path = Path(destination) / source["file"]
    assert hashlib.sha256(path.read_bytes()).hexdigest() == source["sha256"], path
for package in packages:
    if package["license"] == "MPL-2.0":
        name = f'{package["name"]}-{package["version"]}'
        with tarfile.open(Path(destination) / f"{name}-source.tar.gz", "w:gz") as archive:
            archive.add(Path(package["manifest_path"]).parent, arcname=name)
PY
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
assert (contents / "Frameworks/Sparkle.framework/Sparkle").is_file()
assert (contents / "Frameworks/Sparkle.framework/Versions/Current").is_symlink()
framework_info = plistlib.loads((contents / "Frameworks/Sparkle.framework/Resources/Info.plist").read_bytes())
assert tuple(map(int, info["LSMinimumSystemVersion"].split("."))) >= tuple(map(int, framework_info["LSMinimumSystemVersion"].split(".")))
assert info["SUFeedURL"].startswith("https://")
assert info["SUVerifyUpdateBeforeExtraction"] and info["SURequireSignedFeed"]
assert not info["SUAllowsAutomaticUpdates"]
for notice in ["LICENSE", "NOTICE", "THIRD_PARTY_NOTICES.md", "THIRD_PARTY_LICENSES.html"]:
    assert (contents / "Resources" / notice).stat().st_size > 0, notice
assert "gpui-kit" in (contents / "Resources/THIRD_PARTY_LICENSES.html").read_text()
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
# Preview builds use ad-hoc signing; distribution additionally requires Apple's ticket.
codesign --verify --deep --strict "$app/Contents/Frameworks/Sparkle.framework"
codesign --force --sign - "$app"
codesign --verify --deep --strict "$app"
if [[ -n "${HIVE_APPLE_TEAM_ID:-}${HIVE_APPLE_API_KEY:-}${HIVE_APPLE_API_KEY_ID:-}${HIVE_APPLE_API_ISSUER_ID:-}" ]]; then
    python3 scripts/sign_macos.py "$app"
fi
rm -rf "$output/Hive.app"
mv "$app" "$output/Hive.app"
echo "Built $output/Hive.app"
if [[ "${1:-debug}" == release ]]; then
    archive="Hive-$version-macos-arm64.zip"
    ditto -c -k --sequesterRsrc --keepParent "$output/Hive.app" "$stage/$archive"
    unzip -tq "$stage/$archive"
    mv "$stage/$archive" "$output/$archive"
    (cd "$output" && shasum -a 256 "$archive" > "$archive.sha256" && shasum -a 256 -c "$archive.sha256")
    echo "Release archive: $output/$archive"
fi
