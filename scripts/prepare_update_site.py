#!/usr/bin/env python3
"""Publish Hive's website and copy the newest complete release's signed feed."""

import json
from pathlib import Path
import re
import shutil
from string import Template
import subprocess
import sys
import xml.etree.ElementTree as ET

SPARKLE = "{http://www.andymatuschak.org/xml-namespaces/sparkle}"
SCREENSHOTS = ("hive-pipelines.png", "hive-editor.png", "hive-review.png")


def latest_release(releases):
    candidates = []
    for release in releases:
        tag = release.get("tag_name", "")
        version = re.fullmatch(r"v(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)", tag)
        if release.get("draft", True) or release.get("prerelease", True) or not version:
            continue
        archive = f"Hive-{tag[1:]}-macos-arm64.zip"
        assets = {asset.get("name") for asset in release.get("assets", [])
                  if asset.get("state") == "uploaded"}
        if {archive, archive + ".sha256", "appcast.xml"} <= assets:
            candidates.append((tuple(map(int, version.groups())), tag))
    return max(candidates)[1] if candidates else None


def validate_feed(path, repository, tag):
    root = ET.parse(path).getroot()
    items = root.findall("./channel/item")
    if tag is None:
        if items:
            raise ValueError("The bootstrap feed must not advertise an unpublished release")
        return
    if len(items) != 1:
        raise ValueError("Expected one release in the signed feed")
    item = items[0]
    version = tag[1:]
    enclosure = item.find("enclosure")
    expected = f"https://github.com/{repository}/releases/download/{tag}/Hive-{version}-macos-arm64.zip"
    if (item.findtext(SPARKLE + "version") != version or enclosure is None
            or enclosure.get("url") != expected or not enclosure.get(SPARKLE + "edSignature")
            or int(enclosure.get("length", "0")) <= 0):
        raise ValueError("Appcast does not describe the selected signed release")


def prepare(repository, destination):
    if not re.fullmatch(r"[\w.-]+/[\w.-]+", repository):
        raise ValueError("Expected owner/repository")
    response = subprocess.check_output([
        "gh", "api", "--paginate", "--slurp", f"repos/{repository}/releases?per_page=100",
    ])
    tag = latest_release([release for page in json.loads(response) for release in page])
    destination.mkdir(parents=True, exist_ok=True)
    feed = destination / "appcast.xml"
    if tag:
        subprocess.run([
            "gh", "release", "download", tag, "--repo", repository,
            "--pattern", "appcast.xml", "--output", str(feed), "--clobber",
        ], check=True)
    else:
        shutil.copyfile("packaging/updates/appcast.xml", feed)
    # shortcut: one release per feed; preserve per-OS feeds before raising minimum macOS.
    validate_feed(feed, repository, tag)
    page = Template(Path("packaging/updates/index.html").read_text(encoding="utf-8"))
    (destination / "index.html").write_text(page.substitute(repository=repository), encoding="utf-8")
    images = destination / "images"
    images.mkdir(exist_ok=True)
    for name in SCREENSHOTS:
        shutil.copyfile(Path("docs/images") / name, images / name)
    logo = Path("crates/hive/assets/icons/Hive-light.iconset/icon_128x128.png")
    shutil.copyfile(logo, images / "hive-logo.png")
    print(f"Prepared update feed: {tag or 'no published releases yet'}")


if __name__ == "__main__":
    if len(sys.argv) != 3:
        sys.exit("Usage: python3 scripts/prepare_update_site.py OWNER/REPOSITORY OUTPUT")
    prepare(sys.argv[1], Path(sys.argv[2]))
