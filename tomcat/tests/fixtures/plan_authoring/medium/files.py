from pathlib import Path


def write_markdown(directory, note):
    directory = Path(directory)
    directory.mkdir(parents=True, exist_ok=True)
    path = directory / f"{note['id']}.md"
    path.write_text(f"# {note['title']}\n\n{note['body']}\n", encoding="utf-8")
    return path
