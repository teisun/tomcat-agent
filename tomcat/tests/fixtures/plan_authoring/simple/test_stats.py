import contextlib
import io
from pathlib import Path
import tempfile
import unittest

from stats import count_file, main


class StatsTests(unittest.TestCase):
    def test_default_output(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "input.txt"
            path.write_text("hello world\nnext\n", encoding="utf-8")
            output = io.StringIO()
            with contextlib.redirect_stdout(output):
                self.assertEqual(main([str(path)]), 0)
            self.assertEqual(output.getvalue(), "lines=2 words=3\n")

    def test_empty_file(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "empty.txt"
            path.touch()
            self.assertEqual(count_file(path), {"lines": 0, "words": 0})

    def test_missing_file(self):
        with tempfile.TemporaryDirectory() as directory:
            errors = io.StringIO()
            with contextlib.redirect_stderr(errors):
                self.assertEqual(main([str(Path(directory) / "missing")]), 1)
            self.assertTrue(errors.getvalue())


if __name__ == "__main__":
    unittest.main()
