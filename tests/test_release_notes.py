"""Regression checks for the release notes staging contract."""

from pathlib import Path
import sys
from tempfile import TemporaryDirectory
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from scripts.release_notes import github_body, read_notes, resolve, select_version


SOURCE = """= roc-native-windows 0.0.1

Built with {roc-version}.

[source,roc]
----
app [main, native] { pf: platform "{platform-url}" }
----

[source,powershell]
----
$tool = '{cli-url}'
----
"""


class ReleaseNotesTests(unittest.TestCase):
    def setUp(self):
        temporary = TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.directory = Path(temporary.name)
        self.note = self.directory / "0.0.1.adoc"
        self.note.write_text(SOURCE, encoding="utf-8")

    def test_selects_version_numerically_and_ignores_template(self):
        for name in ("template", "0.0.9", "0.0.10-rc.2", "0.0.10-rc.10"):
            (self.directory / f"{name}.adoc").touch()
        self.assertEqual(select_version(self.directory, ""), "0.0.10-rc.10")
        (self.directory / "0.0.10.adoc").touch()
        self.assertEqual(select_version(self.directory, ""), "0.0.10")
        self.assertEqual(select_version(self.directory, "0.0.1"), "0.0.1")

    def test_requires_reviewed_source_and_known_bindings(self):
        with self.assertRaisesRegex(ValueError, "Missing release notes"):
            read_notes(self.directory, "0.0.2")
        for text, error in [
            (SOURCE.replace("0.0.1", "0.0.2"), "must open with"),
            (SOURCE.replace("{cli-url}", "{other-url}"), "unknown bindings"),
            (SOURCE + "\n// author prompt\n", "author prompt"),
            (SOURCE.removesuffix("----\n"), "unclosed source block"),
        ]:
            with self.subTest(error=error), self.assertRaisesRegex(ValueError, error):
                self.note.write_text(text, encoding="utf-8")
                read_notes(self.directory, "0.0.1")

    def test_binds_exact_staged_assets(self):
        platform = "roc-native-windows-0_0_1-" + "P" * 43 + ".tar.zst"
        cli = "windows-0_0_1-" + "C" * 43 + ".tar.zst"
        (self.directory / platform).touch()
        (self.directory / cli).touch()
        base = "https://example.test/releases/download/0.0.1"
        args = ("0.0.1", self.directory, base, "Roc compiler version nightly-2026-11-26-890d6c8")
        rendered = resolve(SOURCE, *args)
        self.assertIn(f"{base}/{platform}", rendered)
        self.assertIn(f"{base}/{cli}", rendered)
        self.assertIn("Built with nightly-2026-11-26-890d6c8.", rendered)
        with self.assertRaisesRegex(ValueError, "Missing staged release asset"):
            resolve(SOURCE + "{checksums-url}", *args)
        (self.directory / cli.replace("C", "D")).touch()
        with self.assertRaisesRegex(ValueError, "exactly one staged bundle"):
            resolve(SOURCE, *args)

    def test_removes_renderer_headings_without_touching_code(self):
        prepared = """# roc-native-windows 0.0.1 {#title}

## Getting started {#getting-started}

```powershell
# Keep this {#example}
roc $tool -- init reading-list
```
"""
        body = github_body(prepared, "0.0.1")
        self.assertTrue(body.startswith("## Getting started\n"))
        self.assertIn("# Keep this {#example}", body)
        self.assertIn("roc $tool -- init reading-list", body)
