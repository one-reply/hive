import hashlib
import importlib.util
import io
from pathlib import Path
import tarfile
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location("install_sparkle", Path(__file__).with_name("install-sparkle.py"))
installer = importlib.util.module_from_spec(spec)
spec.loader.exec_module(installer)


class SparkleInstallTests(unittest.TestCase):
    def test_extracts_prefixed_paths_and_preserves_framework_symlinks(self):
        archive = io.BytesIO()
        # Names and symlink targets captured from the pinned Sparkle 2.10.1 archive.
        with tarfile.open(fileobj=archive, mode="w:xz") as package:
            for name in ["./Sparkle.framework/Versions/B/Sparkle", "./bin/generate_appcast", "./LICENSE"]:
                item = tarfile.TarInfo(name)
                item.size, item.mode = 4, 0o755
                package.addfile(item, io.BytesIO(b"test"))
            for name, target in [("./Sparkle.framework/Versions/Current", "B"),
                                 ("./Sparkle.framework/Sparkle", "Versions/Current/Sparkle")]:
                item = tarfile.TarInfo(name)
                item.type, item.linkname = tarfile.SYMTYPE, target
                package.addfile(item)
        payload = archive.getvalue()

        def download(command, **kwargs):
            Path(command[-1]).write_bytes(payload)

        with tempfile.TemporaryDirectory() as directory, \
                patch.object(installer.subprocess, "run", side_effect=download), \
                patch.object(installer, "SHA256", hashlib.sha256(payload).hexdigest()):
            destination = Path(directory) / "sparkle"
            installer.install(destination)
            self.assertEqual((destination / "Sparkle.framework/Sparkle").read_bytes(), b"test")
            self.assertTrue((destination / "Sparkle.framework/Sparkle").is_symlink())


if __name__ == "__main__":
    unittest.main()
