import argparse
from pathlib import Path
import sys


def count_file(path):
    text = Path(path).read_text(encoding="utf-8")
    return {"lines": len(text.splitlines()), "words": len(text.split())}


def main(argv=None):
    parser = argparse.ArgumentParser()
    parser.add_argument("path")
    args = parser.parse_args(argv)
    try:
        counts = count_file(args.path)
    except (OSError, UnicodeError) as error:
        print(str(error), file=sys.stderr)
        return 1
    print(f"lines={counts['lines']} words={counts['words']}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
