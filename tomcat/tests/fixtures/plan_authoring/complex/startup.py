from pathlib import Path

from storage import IndexStorage


def open_index(path):
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    if not path.exists():
        path.write_text("{}", encoding="utf-8")
    return IndexStorage(path)
