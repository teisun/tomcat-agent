#!/usr/bin/env python3
"""Backfill top-level MessageEntry.id in a tomcat JSONL transcript (pi-mono/OpenAI-style message rows).

Only rows with type == \"message\" and id null/missing/empty get a new id:
  \"{unix_micros_from_row_timestamp}_{monotonic_seq}\"

Rows that already have a non-empty string id are unchanged. First line (session header) is unchanged.

Usage:
  python3 scripts/backfill_transcript_message_ids.py ~/.tomcat/agents/main/sessions/foo.jsonl
"""

from __future__ import annotations

import argparse
import json
import shutil
from datetime import datetime, timezone
from pathlib import Path


def timestamp_to_micros(ts: str) -> int:
    s = ts.strip()
    if s.endswith("Z"):
        s = s[:-1] + "+00:00"
    dt = datetime.fromisoformat(s)
    if dt.tzinfo is None:
        dt = dt.replace(tzinfo=timezone.utc)
    return int(dt.timestamp() * 1_000_000)


def needs_id(obj: dict) -> bool:
    if obj.get("type") != "message":
        return False
    v = obj.get("id")
    if v is None:
        return True
    if isinstance(v, str) and v.strip() == "":
        return True
    return False


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("jsonl", type=Path, help="Path to .jsonl transcript")
    ap.add_argument(
        "--no-backup",
        action="store_true",
        help="Do not write a .bak copy before overwriting",
    )
    args = ap.parse_args()
    path: Path = args.jsonl.expanduser().resolve()
    if not path.is_file():
        raise SystemExit(f"not a file: {path}")

    raw_lines = path.read_text(encoding="utf-8").splitlines()
    out_lines: list[str] = []
    seq = 0

    for i, line in enumerate(raw_lines):
        stripped = line.strip()
        if not stripped:
            out_lines.append(line)
            continue
        try:
            obj = json.loads(stripped)
        except json.JSONDecodeError as e:
            raise SystemExit(f"line {i + 1}: invalid JSON: {e}") from e

        if isinstance(obj, dict) and needs_id(obj):
            ts = obj.get("timestamp")
            if not ts or not isinstance(ts, str):
                raise SystemExit(f"line {i + 1}: message missing timestamp for id backfill")
            micros = timestamp_to_micros(ts)
            obj["id"] = f"{micros}_{seq}"
            seq += 1
            out_lines.append(
                json.dumps(obj, ensure_ascii=False, separators=(",", ":"))
            )
        else:
            out_lines.append(stripped)

    if seq == 0:
        print("No message rows needed id backfill; file unchanged.")
        return

    if not args.no_backup:
        bak = path.with_suffix(path.suffix + ".bak")
        shutil.copy2(path, bak)
        print(f"Backup: {bak}")

    path.write_text("\n".join(out_lines) + "\n", encoding="utf-8")
    print(f"Wrote {path}; assigned ids to {seq} message row(s).")


if __name__ == "__main__":
    main()
