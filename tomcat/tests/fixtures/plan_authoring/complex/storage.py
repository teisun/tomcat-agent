import json
from pathlib import Path


class IndexStorage:
    def __init__(self, path):
        self.path = Path(path)

    def read(self):
        text = self.path.read_text(encoding="utf-8")
        return json.loads(text) if text else {}

    def rebuild(self, documents):
        with self.path.open("w", encoding="utf-8") as stream:
            entries = {}
            for document_id, text in documents:
                entries[document_id] = text
            json.dump(entries, stream, ensure_ascii=False)
