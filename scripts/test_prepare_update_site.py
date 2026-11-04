import tempfile
import unittest
from pathlib import Path

from prepare_update_site import latest_release, validate_feed


def release(version, **fields):
    archive = f"Hive-{version}-macos-arm64.zip"
    return {
        "tag_name": f"v{version}", "draft": False, "prerelease": True,
        "assets": [{"name": name, "state": "uploaded"}
                   for name in [archive, archive + ".sha256", "appcast.xml"]],
        **fields,
    }


class UpdateSiteTests(unittest.TestCase):
    def test_selects_newest_numeric_version_including_published_previews(self):
        self.assertEqual(latest_release([
            release("0.9.0"), release("0.10.0"), release("0.8.0"),
            release("1.0.0", draft=True), release("2.0.0", assets=[]),
            release("3.0.0", tag_name="../../../unexpected"), {},
        ]), "v0.10.0")

    def test_no_complete_release_uses_bootstrap(self):
        self.assertIsNone(latest_release([release("0.1.0", draft=True)]))
        self.assertIsNone(latest_release([]))

    def test_rejects_mismatched_or_unsigned_download_without_rewriting_feed(self):
        content = '''<rss xmlns:sparkle="http://www.andymatuschak.org/xml-namespaces/sparkle">
        <channel><item><sparkle:version>0.1.0</sparkle:version>
        <enclosure url="https://github.com/vitaliits-stack/hive/releases/download/v0.1.0/Hive-0.1.0-macos-arm64.zip"
        sparkle:edSignature="signed" length="42"/></item></channel></rss>'''
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "appcast.xml"
            path.write_text(content)
            validate_feed(path, "vitaliits-stack/hive", "v0.1.0")
            self.assertEqual(path.read_text(), content)
            for repository, tag in [("other/repo", "v0.1.0"), ("vitaliits-stack/hive", "v0.2.0"),
                                    ("vitaliits-stack/hive", None)]:
                with self.assertRaises(ValueError):
                    validate_feed(path, repository, tag)
            path.write_text(content.replace('sparkle:edSignature="signed"', ''))
            with self.assertRaises(ValueError):
                validate_feed(path, "vitaliits-stack/hive", "v0.1.0")


if __name__ == "__main__":
    unittest.main()
