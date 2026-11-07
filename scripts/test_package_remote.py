import importlib.util
import hashlib
import json
from pathlib import Path
import struct
import tempfile
import unittest

spec = importlib.util.spec_from_file_location("package_remote", Path(__file__).with_name("package-remote.py"))
package_remote = importlib.util.module_from_spec(spec)
spec.loader.exec_module(package_remote)


class PackageRemoteTests(unittest.TestCase):
    def fixture(self, path, machine, segment=1):
        data = bytearray(120)
        data[:6] = b"\x7fELF\x02\x01"
        struct.pack_into("<H", data, 18, machine)
        struct.pack_into("<Q", data, 32, 64)
        struct.pack_into("<HH", data, 54, 56, 1)
        struct.pack_into("<I", data, 64, segment)
        path.write_bytes(data)
        return path

    def test_packages_both_architectures_with_matching_checksums(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            binaries = {name: self.fixture(root / name, machine) for name, machine in package_remote.MACHINES.items()}
            package_remote.package(root / "out", "1.2.3", binaries)
            manifest = json.loads((root / "out/manifest.json").read_text())
            self.assertEqual(manifest["version"], "1.2.3")
            self.assertEqual(len(manifest["artifacts"]), 2)
            for artifact in manifest["artifacts"]:
                self.assertEqual(artifact["sha256"], hashlib.sha256((root / "out" / artifact["file"]).read_bytes()).hexdigest())

    def test_rejects_wrong_architecture_dynamic_or_truncated_elf(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "helper"
            for machine, segment in [(183, 1), (62, 3)]:
                self.fixture(path, machine, segment)
                with self.assertRaises(ValueError):
                    package_remote.validate(path, "x86_64")
            path.write_bytes(b"\x7fELF")
            with self.assertRaises(ValueError):
                package_remote.validate(path, "x86_64")
