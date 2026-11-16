import json
from pathlib import Path
import tempfile
import unittest

from cli import export_selection


class ExportTests(unittest.TestCase):
    def test_export_selected_note(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "notes.json"
            source.write_text(json.dumps([
                {"id": "b", "title": "Beta", "body": "B text"},
                {"id": "a", "title": "Alpha", "body": "A text"},
            ]), encoding="utf-8")
            output = root / "export"
            paths = export_selection(source, output, ["a"])
            self.assertEqual([path.name for path in paths], ["a.md"])
            self.assertEqual(paths[0].read_text(encoding="utf-8"), "# Alpha\n\nA text\n")
            self.assertFalse((output / "b.md").exists())


if __name__ == "__main__":
    unittest.main()
