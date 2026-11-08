#!/usr/bin/env python3
"""Verify signed assets, publish a stable release, and refresh the update feed."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import plistlib
import re
import subprocess
import tempfile
import xml.etree.ElementTree as ET

from prepare_update_site import SPARKLE, validate_feed


def verify_assets(directory, repository, tag):
    archive = directory / f"Hive-{tag[1:]}-macos-arm64.zip"
    checksum = archive.with_suffix(".zip.sha256")
    feed = directory / "appcast.xml"
    with archive.open("rb") as stream:
        digest = hashlib.file_digest(stream, "sha256").hexdigest()
    if checksum.read_text().split() != [digest, archive.name]:
        raise ValueError("Release archive checksum does not match")
    validate_feed(feed, repository, tag)
    enclosure = ET.parse(feed).find("./channel/item/enclosure")
    if int(enclosure.get("length")) != archive.stat().st_size:
        raise ValueError("Appcast archive length does not match")
    key = os.environ["SPARKLE_PRIVATE_KEY"]
    for path, signature in [(feed, []), (archive, [enclosure.get(SPARKLE + "edSignature")])]:
        subprocess.run(["target/sparkle/bin/sign_update", "--ed-key-file", "-", "--verify",
                        str(path), *signature], input=key, text=True, check=True)
    with tempfile.TemporaryDirectory(prefix="hive-release-check-") as temporary:
        subprocess.run(["ditto", "-x", "-k", str(archive), temporary], check=True)
        app = Path(temporary) / "Hive.app"
        info = plistlib.loads((app / "Contents/Info.plist").read_bytes())
        expected = plistlib.loads(Path("packaging/macos/Info.plist").read_bytes())
        if (info.get("CFBundleIdentifier") != "dev.hive.Hive"
                or info.get("CFBundleVersion") != tag[1:]
                or info.get("CFBundleShortVersionString") != tag[1:]
                or info.get("SUPublicEDKey") != expected["SUPublicEDKey"]
                or info.get("SUFeedURL") != expected["SUFeedURL"]
                or not info.get("SURequireSignedFeed") or not info.get("SUVerifyUpdateBeforeExtraction")):
            raise ValueError("Release bundle identity, version or update policy does not match")
        for command in [["codesign", "--verify", "--deep", "--strict", str(app)],
                        ["xcrun", "stapler", "validate", str(app)],
                        ["spctl", "--assess", "--type", "execute", "--verbose=2", str(app)]]:
            subprocess.run(command, check=True)
        identity = subprocess.check_output(["codesign", "-dv", "--verbose=4", str(app)],
                                           stderr=subprocess.STDOUT, text=True)
        if (f"TeamIdentifier={os.environ['HIVE_APPLE_TEAM_ID']}" not in identity
                or "Authority=Developer ID Application:" not in identity or "runtime" not in identity):
            raise ValueError("Release is not signed by the configured Developer ID team")
        architectures = subprocess.check_output(["lipo", "-archs", str(app / "Contents/MacOS/hive")], text=True)
        if architectures.strip() != "arm64":
            raise ValueError("Release must contain only Apple Silicon code")
    return [archive, checksum, feed]


def publish(repository, tag, directory, verify_only=False):
    if not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9-]*/[A-Za-z0-9_][A-Za-z0-9_.-]*", repository):
        raise ValueError("Expected owner/repository")
    if not re.fullmatch(r"v(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)", tag):
        raise ValueError("Expected a stable vMAJOR.MINOR.PATCH tag")
    assets = verify_assets(directory, repository, tag)
    if verify_only:
        print("Signing verification passed; no release or update feed was changed")
        return
    # Listing avoids treating authentication/network failures as a missing release.
    pages = json.loads(subprocess.check_output([
        "gh", "api", "--paginate", "--slurp", f"repos/{repository}/releases?per_page=100"]))
    existing = next((item for page in pages for item in page if item["tag_name"] == tag), None)
    published = existing is not None and not existing["draft"]
    if published and existing["prerelease"]:
        raise ValueError("Existing published prerelease requires an explicit promotion")
    if not published:
        notes = Path("docs/releases") / f"{tag}.md"
        if not notes.is_file():
            raise ValueError(f"Missing release notes: {notes}")
        if existing is None:
            subprocess.run(["gh", "release", "create", tag, "--repo", repository, "--verify-tag", "--draft",
                            "--title", f"Hive {tag[1:]}", "--notes-file", str(notes)], check=True)
        subprocess.run(["gh", "release", "upload", tag, "--repo", repository, "--clobber",
                        *map(str, assets)], check=True)
    with tempfile.TemporaryDirectory(prefix="hive-release-download-") as temporary:
        download = Path(temporary)
        subprocess.run(["gh", "release", "download", tag, "--repo", repository, "--dir", temporary], check=True)
        if not published:
            for asset in assets:
                if asset.read_bytes() != (download / asset.name).read_bytes():
                    raise ValueError(f"GitHub asset differs from verified upload: {asset.name}")
        verify_assets(download, repository, tag)
    if not published:
        subprocess.run(["gh", "release", "edit", tag, "--repo", repository, "--draft=false",
                        "--prerelease=false", "--latest", "--notes-file", str(notes)], check=True)
    # GITHUB_TOKEN release events do not trigger other workflows; Pages requires main.
    subprocess.run(["gh", "workflow", "run", "updates.yml", "--repo", repository, "--ref", "main"], check=True)
    print(f"Verified stable release {tag}; update feed dispatched from main")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("repository")
    parser.add_argument("tag")
    parser.add_argument("directory", type=Path)
    parser.add_argument("--verify-only", action="store_true")
    args = parser.parse_args()
    publish(args.repository, args.tag, args.directory, args.verify_only)
