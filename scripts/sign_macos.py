#!/usr/bin/env python3
"""Sign a staged Hive.app with Developer ID and staple Apple's notarization ticket."""

import argparse
import json
import os
from pathlib import Path
import plistlib
import subprocess
import tempfile


def sign_and_notarize(app, identity, profile, keychain=None):
    if not identity.startswith("Developer ID Application: ") or not profile:
        raise ValueError("Set HIVE_SIGNING_IDENTITY to a Developer ID Application identity and HIVE_NOTARY_PROFILE to a notarytool Keychain profile")
    app = Path(app).resolve(strict=True)
    info = plistlib.loads((app / "Contents/Info.plist").read_bytes())
    if info.get("CFBundleIdentifier") != "dev.hive.Hive" or info.get("CFBundleExecutable") != "hive":
        raise ValueError("Expected a staged Hive.app (dev.hive.Hive)")
    framework = app / "Contents/Frameworks/Sparkle.framework"
    version = framework / "Versions/Current"
    # Sign nested executables before their enclosing bundles; never use --deep to sign.
    targets = [version / "XPCServices/Downloader.xpc", version / "XPCServices/Installer.xpc",
               version / "Autoupdate", version / "Updater.app", framework, app]
    for target in [app / "Contents/MacOS/hive", *targets]:
        if not target.resolve(strict=True).is_relative_to(app):
            raise ValueError(f"Signing target escapes Hive.app: {target}")
    keychain_args = ["--keychain", str(keychain)] if keychain else []
    subprocess.run(["codesign", "--verify", "--deep", "--strict", str(app)], check=True)
    for target in targets:
        command = ["codesign", "--force", "--sign", identity, "--options", "runtime",
                   "--timestamp", *keychain_args]
        if target != app:
            command.append("--preserve-metadata=entitlements")
        subprocess.run([*command, str(target)], check=True)
    subprocess.run(["codesign", "--verify", "--deep", "--strict", str(app)], check=True)
    with tempfile.TemporaryDirectory(prefix="hive-notary-") as directory:
        archive = Path(directory) / "Hive.zip"
        subprocess.run(["ditto", "-c", "-k", "--sequesterRsrc", "--keepParent",
                        str(app), str(archive)], check=True)
        result = subprocess.run(["xcrun", "notarytool", "submit", str(archive),
                                 "--keychain-profile", profile, *keychain_args,
                                 "--wait", "--timeout", "30m", "--output-format", "json"],
                                check=True, stdout=subprocess.PIPE, text=True)
        submission = json.loads(result.stdout)
        if not isinstance(submission, dict):
            raise ValueError("Notary service returned an unexpected response")
        if submission.get("status") != "Accepted":
            raise ValueError(f"Notarization not accepted: {submission.get('status')}; submission {submission.get('id')}")
        print(f"Apple accepted notarization: {submission.get('id')}")
    subprocess.run(["xcrun", "stapler", "staple", str(app)], check=True)
    subprocess.run(["xcrun", "stapler", "validate", str(app)], check=True)
    subprocess.run(["codesign", "--verify", "--deep", "--strict", str(app)], check=True)
    subprocess.run(["spctl", "--assess", "--type", "execute", "--verbose=2", str(app)], check=True)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("app", type=Path)
    args = parser.parse_args()
    try:
        sign_and_notarize(args.app, os.environ.get("HIVE_SIGNING_IDENTITY", ""),
                          os.environ.get("HIVE_NOTARY_PROFILE", ""),
                          os.environ.get("HIVE_SIGNING_KEYCHAIN"))
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        parser.exit(1, f"Signing/notarization failed: {error}\n")
