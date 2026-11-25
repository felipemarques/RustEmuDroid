import unittest
from pathlib import Path

from release_notes import render


class ReleaseNoteLinks(unittest.TestCase):
    repository = "https://github.com/example/emulator"
    root = repository + "/blob/123abc/"

    def render(self, text):
        return render(text, Path("docs/releases/v1.md"), self.repository, "123abc")

    def test_manuals_evidence_and_reference_definitions(self):
        notes = (
            "[Manual](../../README.md) · [FPS](../game.md#fps)\n"
            '[Manual em português]: </docs/Manual PT.md> "PT"\n'
        )
        self.assertEqual(
            self.render(notes),
            f"[Manual]({self.root}README.md) · [FPS]({self.root}docs/game.md#fps)\n"
            f'[Manual em português]: <{self.root}docs/Manual%20PT.md> "PT"\n',
        )

    def test_examples_anchors_and_external_links_stay_verbatim(self):
        notes = (
            "`[inline](../a)` [web](https://example.com/a) [section](#fps)\n"
            "[host](//example.com/a)\n"
            "```md\n[example](../b)\n```\n"
            "    [example](../c)\n"
        )
        self.assertEqual(self.render(notes), notes)

    def test_invalid_repository_path_fails_before_publication(self):
        with self.assertRaises(ValueError):
            self.render("[invalid](../../../private.md)")
