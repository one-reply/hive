#!/usr/bin/env python3
"""Validate static Linux helpers and package them with the desktop release manifest."""
import argparse
import hashlib
import json
from pathlib import Path
import shutil
import struct
import tomllib

PROTOCOL = 1
MACHINES = {"x86_64": 62, "aarch64": 183}
MAX_BYTES = 64 * 1024 * 1024


def validate(binary: Path, architecture: str) -> bytes:
    if binary.stat().st_size > MAX_BYTES:
        raise ValueError(f"Helper exceeds 64 MiB: {binary}")
    data = binary.read_bytes()
    if len(data) < 64 or data[:6] != b"\x7fELF\x02\x01":
        raise ValueError(f"Expected a little-endian ELF64 helper: {binary}")
    if struct.unpack_from("<H", data, 18)[0] != MACHINES[architecture]:
        raise ValueError(f"Wrong ELF architecture: {binary}")
    offset = struct.unpack_from("<Q", data, 32)[0]
    entry_size, count = struct.unpack_from("<HH", data, 54)
    if entry_size < 56 or not count or offset + entry_size * count > len(data):
        raise ValueError(f"Invalid ELF program headers: {binary}")
    for index in range(count):
        if struct.unpack_from("<I", data, offset + index * entry_size)[0] == 3:
            raise ValueError(f"Helper must not require a dynamic loader: {binary}")
    return data


def package(destination: Path, version: str, binaries: dict[str, Path]) -> None:
    if set(binaries) != set(MACHINES):
        raise ValueError("Both Linux architectures are required")
    validated = {architecture: validate(path, architecture) for architecture, path in binaries.items()}
    destination.mkdir(parents=True, exist_ok=True)
    artifacts = []
    for architecture, data in validated.items():
        name = f"hive-remote-linux-{architecture}"
        output = destination / name
        shutil.copyfile(binaries[architecture], output)
        output.chmod(0o755)
        artifacts.append({"architecture": architecture, "file": name, "sha256": hashlib.sha256(data).hexdigest()})
    manifest = {"version": version, "protocol": PROTOCOL, "artifacts": artifacts}
    (destination / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--x86_64", type=Path, required=True)
    parser.add_argument("--aarch64", type=Path, required=True)
    parser.add_argument("--output", type=Path, default=Path("target/remote"))
    arguments = parser.parse_args()
    root = Path(__file__).resolve().parent.parent
    version = tomllib.loads((root / "Cargo.toml").read_text())["workspace"]["package"]["version"]
    package(arguments.output, version, {architecture: getattr(arguments, architecture) for architecture in MACHINES})
