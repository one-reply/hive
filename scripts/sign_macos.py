#!/usr/bin/env python3
"""Cloud-sign a staged Hive.app with Xcode and export its stapled notarization ticket."""

import argparse
from datetime import datetime, timezone
import os
from pathlib import Path
import plistlib
import re
import shutil
import subprocess
import tempfile
import time


def prepare_archive(app, team, directory):
    info = plistlib.loads((app / "Contents/Info.plist").read_bytes())
    if not isinstance(info, dict) or info.get("CFBundleIdentifier") != "dev.hive.Hive" or info.get("CFBundleExecutable") != "hive":
        raise ValueError("Expected a staged Hive.app (dev.hive.Hive)")
    for path in app.rglob("*"):
        if path.is_symlink() and not path.resolve(strict=True).is_relative_to(app):
            raise ValueError(f"Symlink escapes Hive.app: {path}")
    subprocess.run(["codesign", "--verify", "--deep", "--strict", str(app)], check=True)
    archive = directory / "Hive.xcarchive"
    copied_app = archive / "Products/Applications/Hive.app"
    copied_app.parent.mkdir(parents=True)
    subprocess.run(["ditto", str(app), str(copied_app)], check=True)
    # Xcode needs platform metadata even for a non-Xcode build.
    info.update(CFBundleSupportedPlatforms=["MacOSX"], DTPlatformName="macosx")
    (copied_app / "Contents/Info.plist").write_bytes(plistlib.dumps(info))
    subprocess.run(["codesign", "--force", "--sign", "-", "--options", "runtime", str(copied_app)], check=True)
    (archive / "Info.plist").write_bytes(plistlib.dumps({
        "ArchiveVersion": 2, "CreationDate": datetime.now(timezone.utc).replace(tzinfo=None),
        "Name": "Hive", "SchemeName": "Hive", "ApplicationProperties": {
            "ApplicationPath": "Applications/Hive.app", "Architectures": ["arm64"],
            "CFBundleIdentifier": info["CFBundleIdentifier"],
            "CFBundleShortVersionString": info["CFBundleShortVersionString"],
            "CFBundleVersion": info["CFBundleVersion"], "SigningIdentity": "-", "Team": team}}))
    options = directory / "ExportOptions.plist"
    options.write_bytes(plistlib.dumps({"method": "developer-id", "destination": "upload",
        "teamID": team, "signingStyle": "automatic", "manageAppVersionAndBuildNumber": False}))
    return archive, options


def export_notarized(archive, destination, authentication, timeout=1800):
    deadline = time.monotonic() + timeout
    while True:
        result = subprocess.run(["xcodebuild", "-exportNotarizedApp", "-archivePath", str(archive),
            "-exportPath", str(destination), "-allowProvisioningUpdates", *authentication],
            stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
        if result.returncode == 0:
            return
        # A pending submission is retried without uploading another copy to Apple.
        if "is processing and not ready for distribution." not in result.stdout:
            raise ValueError(f"Notarized export failed: {result.stdout}")
        if time.monotonic() >= deadline:
            raise TimeoutError(f"Apple is still processing {archive}; resume export when ready")
        print("Apple is processing notarization; checking again in 30 seconds", flush=True)
        time.sleep(30)


def sign_and_notarize(app, team, key=None, key_id=None, issuer=None):
    if not re.fullmatch(r"[A-Z0-9]{10}", team):
        raise ValueError("Set HIVE_APPLE_TEAM_ID to the authorized ten-character Apple team ID")
    if any([key, key_id, issuer]) and not all([key, key_id, issuer]):
        raise ValueError("Set all three HIVE_APPLE_API_KEY, HIVE_APPLE_API_KEY_ID and HIVE_APPLE_API_ISSUER_ID variables")
    authentication = []
    if key:
        key = Path(key).resolve(strict=True)
        if not key.is_file():
            raise ValueError("HIVE_APPLE_API_KEY must point to a private .p8 file")
        authentication = ["-authenticationKeyPath", str(key), "-authenticationKeyID", key_id,
                          "-authenticationKeyIssuerID", issuer]
    app = Path(app).resolve(strict=True)
    directory = Path(tempfile.mkdtemp(prefix="hive-notarization-"))
    try:
        archive, options = prepare_archive(app, team, directory)
        print(f"Signing and uploading to Apple; log: {directory / 'upload.log'}", flush=True)
        with (directory / "upload.log").open("w") as log:
            subprocess.run(["xcodebuild", "-exportArchive", "-archivePath", str(archive),
                "-exportPath", str(directory / "upload"), "-exportOptionsPlist", str(options),
                "-allowProvisioningUpdates", *authentication], check=True, stdout=log, stderr=log)
        destination = directory / "notarized"
        export_notarized(archive, destination, authentication)
        signed_app = destination / "Hive.app"
        subprocess.run(["xcrun", "stapler", "validate", str(signed_app)], check=True)
        subprocess.run(["codesign", "--verify", "--deep", "--strict", str(signed_app)], check=True)
        subprocess.run(["spctl", "--assess", "--type", "execute", "--verbose=2", str(signed_app)], check=True)
        subprocess.run(["ditto", str(signed_app), str(app)], check=True)
        subprocess.run(["codesign", "--verify", "--deep", "--strict", str(app)], check=True)
    except Exception:
        print(f"Preserved Xcode archive and logs for diagnosis/resume: {directory}", flush=True)
        raise
    else:
        shutil.rmtree(directory)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("app", type=Path)
    args = parser.parse_args()
    try:
        sign_and_notarize(args.app, os.environ.get("HIVE_APPLE_TEAM_ID", ""),
                          os.environ.get("HIVE_APPLE_API_KEY"), os.environ.get("HIVE_APPLE_API_KEY_ID"),
                          os.environ.get("HIVE_APPLE_API_ISSUER_ID"))
    except (OSError, ValueError, KeyError, subprocess.CalledProcessError) as error:
        parser.exit(1, f"Signing/notarization failed: {error}\n")
