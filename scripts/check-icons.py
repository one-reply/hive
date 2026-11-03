#!/usr/bin/env python3
"""Check the committed macOS icon exports using only the standard library."""
import json
import struct
from pathlib import Path

icons = Path(__file__).resolve().parents[1] / "crates/hive/assets/icons"
document = json.loads((icons / "Hive.icon/icon.json").read_text())
for group in document["groups"]:
    for layer in group["layers"]:
        assert (icons / "Hive.icon/Assets" / layer["image-name"]).is_file()

for appearance in ("light", "dark"):
    expected = {f"hive-{appearance}.png": 1024}
    for size in (16, 32, 128, 256, 512):
        for scale in (1, 2):
            suffix = "@2x" if scale == 2 else ""
            expected[f"Hive-{appearance}.iconset/icon_{size}x{size}{suffix}.png"] = size * scale
    for name, size in expected.items():
        data = (icons / name).read_bytes()
        assert data[:8] == b"\x89PNG\r\n\x1a\n", name
        assert struct.unpack(">II", data[16:24]) == (size, size), name
        assert data[25] in (4, 6), f"{name}: transparency is required"

    data = (icons / f"Hive-{appearance}.icns").read_bytes()
    assert data[:4] == b"icns"
    assert struct.unpack(">I", data[4:8])[0] == len(data)
    chunks, offset = set(), 8
    while offset < len(data):
        kind, length = struct.unpack(">4sI", data[offset:offset + 8])
        assert length >= 8 and offset + length <= len(data)
        chunks.add(kind)
        offset += length
    assert offset == len(data)
    assert {b"ic07", b"ic08", b"ic09", b"ic10", b"ic11", b"ic12", b"ic13", b"ic14"} <= chunks
    assert any(small <= chunks for small in (
        {b"is32", b"s8mk", b"il32", b"l8mk"}, {b"icp4", b"icp5"}, {b"ic04", b"ic05"},
    ))

assert (icons / "hive-light.png").read_bytes() != (icons / "hive-dark.png").read_bytes()
print("Verified: light/dark masters, 20 Retina/standard PNGs, and both ICNS containers.")
