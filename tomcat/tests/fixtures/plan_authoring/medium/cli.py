import argparse
import sys

from files import write_markdown
from notes import load_notes, selected_notes


def export_selection(source, destination, ids):
    notes = load_notes(source)
    return [write_markdown(destination, note) for note in selected_notes(notes, ids)]


def main(argv=None):
    parser = argparse.ArgumentParser()
    parser.add_argument("source")
    parser.add_argument("destination")
    parser.add_argument("ids", nargs="+")
    args = parser.parse_args(argv)
    for path in export_selection(args.source, args.destination, args.ids):
        print(path.name)
    return 0


if __name__ == "__main__":
    sys.exit(main())
