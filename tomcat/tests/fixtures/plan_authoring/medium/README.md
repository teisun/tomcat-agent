# Notes export CLI

Python 3, standard library only. Run `python3 -m unittest -v`.

`python3 cli.py NOTES_JSON OUTPUT_DIR ID [ID ...]` exports selected notes. The JSON is a list of objects with stable string `id`, `title`, and `body`. Existing modules separate loading/selection, writing one Markdown file, and the CLI. Only happy-path export is currently covered.
