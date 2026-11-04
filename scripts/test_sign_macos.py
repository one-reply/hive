from pathlib import Path
import plistlib
import shutil
import subprocess
import tempfile
import unittest
from unittest.mock import patch

from sign_macos import export_notarized, prepare_archive, sign_and_notarize


class DeveloperIDTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name).resolve()
        self.app = self.root / "Hive.app"
        (self.app / "Contents").mkdir(parents=True)
        self.info = {"CFBundleIdentifier": "dev.hive.Hive", "CFBundleExecutable": "hive",
                     "CFBundleVersion": "0.1.1", "CFBundleShortVersionString": "0.1.1"}
        (self.app / "Contents/Info.plist").write_bytes(plistlib.dumps(self.info))

    def test_partial_credentials_fail_before_modifying_app(self):
        with patch("sign_macos.subprocess.run") as runner, self.assertRaises(ValueError):
            sign_and_notarize(self.app, "TEAM123456", key_id="incomplete")
        runner.assert_not_called()

    def test_symlink_escape_rejected_before_modifying_app(self):
        (self.app / "Contents/escape").symlink_to(self.root)
        with patch("sign_macos.subprocess.run") as runner, self.assertRaises(ValueError):
            prepare_archive(self.app, "TEAM123456", self.root / "export")
        runner.assert_not_called()

    def test_archive_preserves_source_and_version(self):
        def run(command, **kwargs):
            if command[0] == "ditto":
                shutil.copytree(command[1], command[2])
        with patch("sign_macos.subprocess.run", side_effect=run):
            archive, options = prepare_archive(self.app, "TEAM123456", self.root / "export")
        self.assertEqual(plistlib.loads((self.app / "Contents/Info.plist").read_bytes()), self.info)
        metadata = plistlib.loads((archive / "Info.plist").read_bytes())
        self.assertEqual(metadata["ApplicationProperties"]["CFBundleVersion"], "0.1.1")
        policy = plistlib.loads(options.read_bytes())
        self.assertEqual(policy["method"], "developer-id")
        self.assertEqual(policy["teamID"], "TEAM123456")
        self.assertFalse(policy["manageAppVersionAndBuildNumber"])

    def test_pending_export_retries_without_uploading_again(self):
        results = [subprocess.CompletedProcess([], 65, 'Archive "Hive.xcarchive" is processing and not ready for distribution.'),
                   subprocess.CompletedProcess([], 0, "Export succeeded")]
        with patch("sign_macos.subprocess.run", side_effect=results) as runner, patch("sign_macos.time.sleep"):
            export_notarized(self.root, self.root / "output", [])
        self.assertEqual(runner.call_count, 2)
        self.assertTrue(all("-exportNotarizedApp" in call.args[0] for call in runner.call_args_list))

    def test_failed_export_does_not_retry(self):
        with patch("sign_macos.subprocess.run", return_value=subprocess.CompletedProcess([], 65, "Rejected")) as runner:
            with self.assertRaisesRegex(ValueError, "Rejected"):
                export_notarized(self.root, self.root / "output", [])
        self.assertEqual(runner.call_count, 1)

    def test_failed_ticket_validation_never_replaces_source(self):
        work = self.root / "signing"
        work.mkdir()
        result = subprocess.CompletedProcess([], 0)
        with patch("sign_macos.tempfile.mkdtemp", return_value=str(work)), \
                patch("sign_macos.prepare_archive", return_value=(work / "Hive.xcarchive", work / "ExportOptions.plist")), \
                patch("sign_macos.export_notarized"), \
                patch("sign_macos.subprocess.run", side_effect=[result, subprocess.CalledProcessError(65, "stapler")]) as runner:
            with self.assertRaises(subprocess.CalledProcessError):
                sign_and_notarize(self.app, "TEAM123456")
        self.assertFalse(any(call.args[0][0] == "ditto" for call in runner.call_args_list))
        self.assertEqual(plistlib.loads((self.app / "Contents/Info.plist").read_bytes()), self.info)
        self.assertTrue((work / "upload.log").exists())

    def test_pending_export_times_out(self):
        result = subprocess.CompletedProcess([], 65, "is processing and not ready for distribution.")
        with patch("sign_macos.subprocess.run", return_value=result), patch("sign_macos.time.sleep") as sleep:
            with self.assertRaises(TimeoutError):
                export_notarized(self.root, self.root / "output", [], timeout=0)
        sleep.assert_not_called()


if __name__ == "__main__":
    unittest.main()
