#!/usr/bin/env python3
"""Install the pinned Sparkle framework and signing tools, preserving symlinks."""

import hashlib
from pathlib import Path
import subprocess
import sys
import tarfile
import tempfile

VERSION = "2.10.1"
SHA256 = "1a856d2a9edb8e628de88121d7be04cf0e2167f8641650b78fca00b14244cf27"


def install(destination):
    destination.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="hive-sparkle-") as temporary:
        archive = Path(temporary) / "Sparkle.tar.xz"
        subprocess.run([
            "curl", "--fail", "--location", "--silent", "--show-error", "--retry", "3",
            f"https://github.com/sparkle-project/Sparkle/releases/download/{VERSION}/Sparkle-{VERSION}.tar.xz",
            "--output", str(archive),
        ], check=True)
        if hashlib.sha256(archive.read_bytes()).hexdigest() != SHA256:
            raise ValueError("Sparkle archive checksum mismatch")
        with tarfile.open(archive) as package:
            members = [item for item in package.getmembers()
                       if item.name.split("/")[0] in {"Sparkle.framework", "bin", "LICENSE"}]
            package.extractall(destination, members=members, filter="data")
    assert (destination / "Sparkle.framework/Sparkle").is_file()
    assert (destination / "Sparkle.framework/Versions/Current").is_symlink()
    assert (destination / "bin/generate_appcast").is_file()
    assert (destination / "LICENSE").stat().st_size > 0
    print(f"Verified Sparkle {VERSION}: {destination}")


if __name__ == "__main__":
    if len(sys.argv) != 2:
        sys.exit("Usage: python3 scripts/install-sparkle.py DIRECTORY")
    install(Path(sys.argv[1]).resolve())
