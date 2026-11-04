import json
from pathlib import Path
import plistlib
import subprocess
import tempfile
import unittest
from unittest.mock import patch

from sign_macos import sign_and_notarize


class DeveloperIDTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.app = Path(self.directory.name).resolve() / "Hive.app"
        contents = self.app / "Contents"
        (contents / "MacOS").mkdir(parents=True)
        (contents / "MacOS/hive").write_bytes(b"fixture")
        (contents / "Info.plist").write_bytes(plistlib.dumps({
            "CFBundleIdentifier": "dev.hive.Hive", "CFBundleExecutable": "hive"}))
        version = contents / "Frameworks/Sparkle.framework/Versions/B"
        for bundle in ["XPCServices/Downloader.xpc", "XPCServices/Installer.xpc", "Updater.app"]:
            (version / bundle).mkdir(parents=True)
        (version / "Autoupdate").write_bytes(b"fixture")
        (version.parent / "Current").symlink_to("B")
        self.identity = "Developer ID Application: Example (TEAM123456)"

    def run_signing(self, status):
        def run(command, **kwargs):
            return subprocess.CompletedProcess(command, 0, json.dumps({"status": status, "id": "submission-id"}))
        with patch("sign_macos.subprocess.run", side_effect=run) as runner:
            sign_and_notarize(self.app, self.identity, "hive-notary")
        return [call.args[0] for call in runner.call_args_list]

    def test_rejects_development_identity_before_modifying_bundle(self):
        with patch("sign_macos.subprocess.run") as runner, self.assertRaises(ValueError):
            sign_and_notarize(self.app, "Apple Development: Example", "hive-notary")
        runner.assert_not_called()

    def test_rejects_nested_symlink_outside_bundle(self):
        helper = self.app / "Contents/Frameworks/Sparkle.framework/Versions/B/Autoupdate"
        helper.unlink()
        helper.symlink_to(Path(self.directory.name))
        with patch("sign_macos.subprocess.run") as runner, self.assertRaises(ValueError):
            sign_and_notarize(self.app, self.identity, "hive-notary")
        runner.assert_not_called()

    def test_failed_signing_does_not_upload(self):
        with patch("sign_macos.subprocess.run", side_effect=subprocess.CalledProcessError(1, "codesign")) as runner:
            with self.assertRaises(subprocess.CalledProcessError):
                sign_and_notarize(self.app, self.identity, "hive-notary")
        self.assertEqual(runner.call_count, 1)

    def test_rejected_notarization_never_staples_or_accepts_app(self):
        for status in ["Invalid", "In Progress", None]:
            with self.subTest(status=status), patch("sign_macos.subprocess.run") as runner:
                runner.return_value = subprocess.CompletedProcess([], 0, json.dumps({"status": status}))
                with self.assertRaises(ValueError):
                    sign_and_notarize(self.app, self.identity, "hive-notary")
                commands = [call.args[0] for call in runner.call_args_list]
                self.assertFalse(any("stapler" in command or command[0] == "spctl" for command in commands))

    def test_accepted_submission_is_stapled_validated_and_assessed(self):
        commands = self.run_signing("Accepted")
        self.assertIn(["xcrun", "stapler", "staple", str(self.app)], commands)
        self.assertIn(["xcrun", "stapler", "validate", str(self.app)], commands)
        self.assertEqual(commands[-1][:4], ["spctl", "--assess", "--type", "execute"])
        signatures = [command for command in commands if "--sign" in command]
        self.assertEqual(signatures[-1][-1], str(self.app))
        for command in signatures:
            self.assertIn("runtime", command)
            self.assertIn("--timestamp", command)
            self.assertNotIn("--deep", command)

    def test_malformed_notary_response_fails_closed(self):
        for response in ['{"status":', '[]', 'null']:
            with self.subTest(response=response), patch("sign_macos.subprocess.run") as runner:
                runner.return_value = subprocess.CompletedProcess([], 0, response)
                with self.assertRaises(ValueError):
                    sign_and_notarize(self.app, self.identity, "hive-notary")
                self.assertFalse(any("stapler" in call.args[0] for call in runner.call_args_list))


if __name__ == "__main__":
    unittest.main()
