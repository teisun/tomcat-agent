#!/usr/bin/env python3
"""Crop the Tomcat sidebar strip from full-frame VSIX visual screenshots.

Reads ``tomcat-vsix-visual-*.png`` screenshots from the artifacts dir, crops a
left strip (activitybar + sidebar webview) and writes ``*-cropped.png`` next to
them. Prints the full-frame paths as a fallback so they can be read directly if
the crop is too narrow.

Width is configurable via env:
  TOMCAT_CROP_WIDTH  logical CSS pixels of the strip (default 520)
  TOMCAT_CROP_DPR     device pixel ratio of screencapture output (default 2)
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
    "tomcat-vsix-visual-expanded.png",
    "tomcat-vsix-visual-file-chip.png",
    "tomcat-vsix-visual-progress.png",
    "tomcat-vsix-visual-tool-icons.png",
    "tomcat-vsix-visual-tool-icons-bottom.png",
    "tomcat-vsix-visual-todo-expanded.png",
)

HEIGHTS = {
    "tomcat-vsix-visual-file-chip.png": 640,
    "tomcat-vsix-visual-tool-icons.png": 1050,
    "tomcat-vsix-visual-tool-icons-bottom.png": 1050,
}


def crop_one(src: Path, width_px: int, height_px: int | None = None) -> Path | None:
    if not src.exists():
        return None
    with Image.open(src) as img:
        w, h = img.size
        crop_w = min(width_px, w)
        crop_h = min(height_px, h) if height_px is not None else h
        cropped = img.crop((0, 0, crop_w, crop_h))
        out = src.with_name(src.stem + "-cropped.png")
        cropped.save(out)
        print(f"cropped: {out} ({crop_w}x{crop_h} from {w}x{h})")
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
        logical_height = HEIGHTS.get(name)
        height_px = logical_height * dpr if logical_height is not None else None
        if crop_one(src, width_px, height_px) is not None:
            produced += 1

    if produced == 0:
        print(
            "no cropped images produced; read full-frame screenshots directly.",
            file=sys.stderr,
        )
        return 3
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
