#!/usr/bin/env python3
"""Install pinned, checksum-verified release tools for an Apple Silicon build host."""

import hashlib
from pathlib import Path
import platform
import subprocess
import sys
import tarfile
import tempfile

TOOLS = [
    ("gitleaks", "gitleaks/gitleaks", "v8.30.1", "gitleaks_8.30.1_darwin_arm64.tar.gz",
     "b40ab0ae55c505963e365f271a8d3846efbc170aa17f2607f13df610a9aeb6a5"),
    ("cargo-deny", "EmbarkStudios/cargo-deny", "0.20.2", "cargo-deny-0.20.2-aarch64-apple-darwin.tar.gz",
     "fe67d82a10d8597a3549364cb733a3f9cc1bfff9031b7ae46384a9f2a72090c3"),
    ("cargo-about", "EmbarkStudios/cargo-about", "0.9.3", "cargo-about-0.9.3-aarch64-apple-darwin.tar.gz",
     "45520df8680978a6e7c4023ace0e3f311c73efc208cd68e5a7f4ed4822fa0349"),
]

if len(sys.argv) != 2 or (platform.system(), platform.machine()) != ("Darwin", "arm64"):
    sys.exit("Usage: python3 scripts/install-release-tools.py DIRECTORY (macOS arm64 only)")
destination = Path(sys.argv[1]).resolve()
destination.mkdir(parents=True, exist_ok=True)
with tempfile.TemporaryDirectory(prefix="hive-tools-") as temporary:
    for name, repository, version, filename, checksum in TOOLS:
        archive = Path(temporary) / filename
        url = f"https://github.com/{repository}/releases/download/{version}/{filename}"
        subprocess.run(["curl", "--fail", "--location", "--silent", "--show-error", "--retry", "3",
                        url, "--output", str(archive)], check=True)
        if hashlib.sha256(archive.read_bytes()).hexdigest() != checksum:
            sys.exit(f"Checksum mismatch for {filename}")
        with tarfile.open(archive) as package:
            matches = [item for item in package.getmembers() if item.isfile() and Path(item.name).name == name]
            if len(matches) != 1:
                sys.exit(f"Expected exactly one {name} executable in {filename}")
            executable = destination / name
            executable.write_bytes(package.extractfile(matches[0]).read())
            executable.chmod(0o755)
        subprocess.run([str(executable), "--version"], check=True)
