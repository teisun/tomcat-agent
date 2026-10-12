#!/usr/bin/env python3
"""Create optional left-window previews from default VSIX full-frame screenshots.

Writes ``*-cropped.png`` next to the originals. A fixed left strip does not locate
Tomcat in every workbench layout; visual acceptance must read the original full
frames together with their AX/console evidence, not rely on these previews.

Width is configurable via env:
  TOMCAT_CROP_WIDTH  logical CSS pixels of the strip (default 520)
  TOMCAT_CROP_DPR     device pixel ratio of CDP output (default 2)
"""

from __future__ import annotations

import argparse
import os
import sys
from pathlib import Path

try:
    from PIL import Image
except ImportError:
    print(
        "Pillow not installed; run `pip3 install Pillow`. "
        "Full-frame screenshots remain available for inspection.",
        file=sys.stderr,
    )
    sys.exit(2)

NAMES = (
    "tomcat-vsix-visual-collapsed.png",
    "tomcat-vsix-visual-progress.png",
    "tomcat-vsix-visual-reload-replay.png",
    "tomcat-vsix-visual-switch-restore.png",
    "tomcat-vsix-visual-todo-expanded.png",
)

# Current default verify:vsix capture set. Legacy owner/expanded/icon captures have
# no producer in this suite; the original full frames remain the visual evidence.


def crop_one(src: Path, width_px: int) -> Path | None:
    if not src.exists():
        return None
    with Image.open(src) as img:
        w, h = img.size
        crop_w = min(width_px, w)
        cropped = img.crop((0, 0, crop_w, h))
        out = src.with_name(src.stem + "-cropped.png")
        cropped.save(out)
        print(f"cropped: {out} ({crop_w}x{h} from {w}x{h})")
        return out


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--artifacts-dir", required=True)
    args = parser.parse_args()

    artifacts = Path(args.artifacts_dir)
    logical_width = int(os.environ.get("TOMCAT_CROP_WIDTH", "520"))
    dpr = int(os.environ.get("TOMCAT_CROP_DPR", "2"))
    width_px = logical_width * dpr

    produced = 0
    for name in NAMES:
        src = artifacts / name
        if not src.exists():
            print(f"full-frame not found: {src}", file=sys.stderr)
            continue
        print(f"full-frame: {src}")
        if crop_one(src, width_px) is not None:
            produced += 1

    if produced != len(NAMES):
        print(
            f"only {produced}/{len(NAMES)} required full frames available; do not count missing captures as accepted.",
            file=sys.stderr,
        )
        return 3
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
