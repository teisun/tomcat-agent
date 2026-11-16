import json
from pathlib import Path


def load_notes(path):
    return {note["id"]: note for note in json.loads(Path(path).read_text(encoding="utf-8"))}


def selected_notes(notes, ids):
    for note_id in ids:
        yield notes[note_id]
