import hashlib
import json
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

from publish_release import publish, verify_assets


class PublishReleaseTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.assets = [self.root / name for name in
                       ['Hive-1.0.1-macos-arm64.zip', 'Hive-1.0.1-macos-arm64.zip.sha256', 'appcast.xml']]
        for path in self.assets:
            path.write_bytes(b'verified signed asset')
        self.calls = []
        self.corrupt = False

    def run_command(self, command, **kwargs):
        self.calls.append(command)
        if command[:3] == ['gh', 'release', 'download']:
            destination = Path(command[command.index('--dir') + 1])
            for path in self.assets:
                (destination / path.name).write_bytes(b'corrupt' if self.corrupt else path.read_bytes())
        return subprocess.CompletedProcess(command, 0)

    def invoke(self, existing=None, verify_only=False):
        pages = [[existing]] if existing else [[]]
        with patch('publish_release.verify_assets', return_value=self.assets) as verify, \
                patch('publish_release.subprocess.check_output', return_value=json.dumps(pages)), \
                patch('publish_release.subprocess.run', side_effect=self.run_command), \
                patch('publish_release.Path.is_file', return_value=True):
            publish('one-reply/hive', 'v1.0.1', self.root, verify_only)
        return verify

    def test_new_release_published_stable_only_after_download_verification(self):
        verify = self.invoke()
        self.assertEqual(verify.call_count, 2)
        create = next(c for c in self.calls if c[:3] == ['gh', 'release', 'create'])
        self.assertIn('--draft', create)
        self.assertIn('--verify-tag', create)
        edit = next(c for c in self.calls if c[:3] == ['gh', 'release', 'edit'])
        for option in ['--draft=false', '--prerelease=false', '--latest']:
            self.assertIn(option, edit)
        self.assertLess(next(i for i, c in enumerate(self.calls) if c[:3] == ['gh', 'release', 'download']),
                        self.calls.index(edit))
        self.assertEqual(self.calls[-1], ['gh', 'workflow', 'run', 'updates.yml', '--repo', 'one-reply/hive', '--ref', 'main'])

    def test_existing_draft_is_resumed_without_duplicate_creation(self):
        self.invoke({'tag_name': 'v1.0.1', 'draft': True, 'prerelease': False})
        self.assertFalse(any(c[:3] == ['gh', 'release', 'create'] for c in self.calls))
        self.assertTrue(any(c[:3] == ['gh', 'release', 'upload'] for c in self.calls))

    def test_published_release_is_verified_without_overwriting_assets_or_metadata(self):
        verify = self.invoke({'tag_name': 'v1.0.1', 'draft': False, 'prerelease': False})
        self.assertEqual(verify.call_count, 2)
        self.assertEqual([c[2] for c in self.calls if c[:2] == ['gh', 'release']], ['download'])
        self.assertEqual(self.calls[-1][-2:], ['--ref', 'main'])

    def test_published_prerelease_requires_explicit_promotion(self):
        with self.assertRaisesRegex(ValueError, 'explicit promotion'):
            self.invoke({'tag_name': 'v1.0.1', 'draft': False, 'prerelease': True})
        self.assertEqual(self.calls, [])

    def test_tampered_download_is_not_published(self):
        self.corrupt = True
        with self.assertRaisesRegex(ValueError, 'differs'):
            self.invoke()
        self.assertFalse(any(c[:3] == ['gh', 'release', 'edit'] for c in self.calls))
        self.assertFalse(any(c[:2] == ['gh', 'workflow'] for c in self.calls))

    def test_verification_mode_has_no_github_side_effects(self):
        verify = self.invoke(verify_only=True)
        self.assertEqual(verify.call_count, 1)
        self.assertEqual(self.calls, [])

    def test_api_failure_is_not_treated_as_missing_release(self):
        with patch('publish_release.verify_assets', return_value=self.assets), \
                patch('publish_release.subprocess.check_output', side_effect=subprocess.CalledProcessError(1, 'gh')), \
                patch('publish_release.subprocess.run') as run:
            with self.assertRaises(subprocess.CalledProcessError):
                publish('one-reply/hive', 'v1.0.1', self.root)
        run.assert_not_called()

    def test_failed_signature_or_notarization_prevents_publication(self):
        with patch('publish_release.verify_assets', side_effect=ValueError('signature rejected')), \
                patch('publish_release.subprocess.run') as run:
            with self.assertRaisesRegex(ValueError, 'signature rejected'):
                publish('one-reply/hive', 'v1.0.1', self.root)
        run.assert_not_called()

    def test_invalid_tag_or_repository_fails_before_verification(self):
        with patch('publish_release.verify_assets') as verify, \
                patch('publish_release.subprocess.check_output') as api:
            for repository, tag in [('one-reply/hive', '../bad'), ('one-reply/hive', 'v1.0.2-beta'),
                                    ('../hive', 'v1.0.1')]:
                with self.assertRaises(ValueError):
                    publish(repository, tag, self.root)
        verify.assert_not_called()
        api.assert_not_called()

    def test_invalid_checksum_fails_before_running_tools(self):
        with patch('publish_release.subprocess.run') as run:
            with self.assertRaisesRegex(ValueError, 'checksum'):
                verify_assets(self.root, 'one-reply/hive', 'v1.0.1')
        run.assert_not_called()

    def test_wrong_feed_length_fails_before_running_tools(self):
        archive, checksum, feed = self.assets
        checksum.write_text(hashlib.sha256(archive.read_bytes()).hexdigest() + '  ' + archive.name + '\n')
        feed.write_text('''<rss xmlns:sparkle="http://www.andymatuschak.org/xml-namespaces/sparkle"><channel><item>
        <sparkle:version>1.0.1</sparkle:version>
        <enclosure url="https://github.com/one-reply/hive/releases/download/v1.0.1/Hive-1.0.1-macos-arm64.zip"
        length="1" sparkle:edSignature="signature"/></item></channel></rss>''')
        with patch('publish_release.subprocess.run') as run:
            with self.assertRaisesRegex(ValueError, 'length'):
                verify_assets(self.root, 'one-reply/hive', 'v1.0.1')
        run.assert_not_called()


if __name__ == '__main__':
    unittest.main()
