from pathlib import Path
import tempfile
import unittest

from query import search
from startup import open_index
from tasks import IndexTask


class IndexTests(unittest.TestCase):
    def test_rebuild_and_reopen(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "index.json"
            storage = open_index(path)
            IndexTask(storage).rebuild([("a", "hello"), ("b", "world")])
            self.assertEqual(search(open_index(path), "hello"), ["a"])

    def test_queries_during_and_after_failed_rebuild(self):
        with tempfile.TemporaryDirectory() as directory:
            storage = open_index(Path(directory) / "index.json")
            task = IndexTask(storage)
            task.rebuild([("old", "hello")])
            observed = []

            def documents():
                observed.append(search(storage, "hello"))
                yield "new", "hello again"
                raise OSError("document source unavailable")

            with self.assertRaises(OSError):
                task.rebuild(documents())
            # Characterization of today's behavior, not the desired recovery contract.
            self.assertEqual(observed, [[]])
            self.assertEqual(search(storage, "hello"), [])


if __name__ == "__main__":
    unittest.main()
