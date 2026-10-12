#!/usr/bin/env python3
"""One-shot migration for byte-free v2 transcripts.

The migration deliberately lives outside the CLI: it is operational tooling, not a
permanent user-facing command. It has no dependencies beyond the Python standard
library and is safe to re-run after an interrupted attempt.

It performs only format relocation:
* inline ``input_image.image_b64`` -> ``input_image_ref.blob_sha`` plus CAS blob;
* inline ``tool_display`` -> ``<session>.tool_display.jsonl``;
* diffs older than seven days -> metadata-only expired summaries.

It never recomputes a diff. New writes are already bounded by the Rust runtime.
"""

from __future__ import annotations

import argparse
import base64
import datetime as dt
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
from dataclasses import dataclass
from typing import Any, Iterable


DIFF_RETENTION = dt.timedelta(days=7)
TEST_ALLOW_RUNNING_SERVE_ENV = "TOMCAT_MIGRATION_TEST_ALLOW_RUNNING_SERVE"
JSON = dict[str, Any]


@dataclass
class FileStats:
    before: int
    after: int
    images: int = 0
    displays: int = 0


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--sessions-dir",
        type=Path,
        default=Path.home() / ".tomcat" / "agents" / "main" / "sessions",
        help="session directory (default: ~/.tomcat/agents/main/sessions)",
    )
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="report changes without writing blobs, sidecars, backups, or transcripts",
    )
    parser.add_argument(
        "--self-test",
        action="store_true",
        help="run an isolated idempotence test and exit",
    )
    return parser.parse_args()


def is_serve_running() -> bool:
    """Reject mutation when any local Tomcat serve process is alive.

    ``ps`` is present on supported desktop platforms. Failure to inspect it is treated as
    a hard stop: proceeding while the append-only writer is alive risks dropping a row.
    """

    try:
        completed = subprocess.run(
            ["ps", "-axo", "pid=,command="],
            check=True,
            capture_output=True,
            text=True,
        )
    except (OSError, subprocess.CalledProcessError) as error:
        raise RuntimeError(f"cannot determine whether tomcat serve is running: {error}") from error

    this_pid = str(os.getpid())
    for line in completed.stdout.splitlines():
        pid, _, command = line.strip().partition(" ")
        if pid == this_pid:
            continue
        if "tomcat" in command and "serve" in command and "--stdio" in command:
            return True
    return False


def session_paths(sessions_dir: Path) -> Iterable[Path]:
    for path in sorted(sessions_dir.glob("*.jsonl")):
        name = path.name
        if name.endswith(".tool_display.jsonl"):
            continue
        yield path


def main_transcript_path(path: Path) -> bool:
    return not path.name.endswith(".user_messages.jsonl")


def tool_display_sidecar_path(transcript_path: Path) -> Path:
    return transcript_path.with_name(f"{transcript_path.stem}.tool_display.jsonl")


def resume_index_path(transcript_path: Path) -> Path:
    return transcript_path.with_name(f"{transcript_path.stem}.resume-index.json")


def parse_timestamp(value: Any) -> dt.datetime | None:
    if not isinstance(value, str):
        return None
    try:
        parsed = dt.datetime.fromisoformat(value.replace("Z", "+00:00"))
    except ValueError:
        return None
    return parsed.astimezone(dt.timezone.utc)


def expired_timestamp(value: Any, now: dt.datetime) -> bool:
    timestamp = parse_timestamp(value)
    return timestamp is not None and timestamp < now - DIFF_RETENTION


def expired_display_summary(display: Any) -> Any:
    if not isinstance(display, dict):
        return display
    summary = dict(display)
    summary["expired"] = True
    if summary.get("kind") == "file":
        summary["diff"] = None
    elif summary.get("kind") == "files" and isinstance(summary.get("files"), list):
        files: list[Any] = []
        for item in summary["files"]:
            if isinstance(item, dict):
                file_summary = dict(item)
                file_summary["diff"] = None
                file_summary["expired"] = True
                files.append(file_summary)
            else:
                files.append(item)
        summary["files"] = files
    return summary


def write_blob(blobs_dir: Path, raw: bytes, dry_run: bool) -> str:
    sha = hashlib.sha256(raw).hexdigest()
    if dry_run:
        return sha
    blobs_dir.mkdir(parents=True, exist_ok=True)
    destination = blobs_dir / sha
    if destination.exists():
        return sha
    # Atomic publication prevents readers from seeing a partially written image.
    fd, temporary = tempfile.mkstemp(prefix=f".{sha}.", dir=blobs_dir)
    try:
        with os.fdopen(fd, "wb") as output:
            output.write(raw)
            output.flush()
            os.fsync(output.fileno())
        try:
            os.replace(temporary, destination)
        except FileExistsError:
            # Another migration instance won the same content-addressed race.
            os.unlink(temporary)
    finally:
        if os.path.exists(temporary):
            os.unlink(temporary)
    return sha


def convert_content_images(content: Any, blobs_dir: Path, dry_run: bool) -> tuple[Any, int]:
    if not isinstance(content, list):
        return content, 0
    converted: list[Any] = []
    count = 0
    for item in content:
        if not isinstance(item, dict) or item.get("type") != "input_image":
            converted.append(item)
            continue
        encoded = item.get("image_b64")
        mime_type = item.get("mime_type")
        if not isinstance(encoded, str) or not isinstance(mime_type, str):
            converted.append(item)
            continue
        try:
            raw = base64.b64decode(encoded, validate=True)
        except (ValueError, TypeError) as error:
            print(f"warning: keeping invalid inline image base64: {error}", file=sys.stderr)
            converted.append(item)
            continue
        replacement: JSON = {
            "type": "input_image_ref",
            "blob_sha": write_blob(blobs_dir, raw, dry_run),
            "mime_type": mime_type,
        }
        if isinstance(item.get("detail"), str):
            replacement["detail"] = item["detail"]
        converted.append(replacement)
        count += 1
    return converted, count


def existing_sidecar_keys(path: Path) -> set[tuple[str, str]]:
    keys: set[tuple[str, str]] = set()
    if not path.exists():
        return keys
    with path.open("rb") as source:
        for line_number, raw_line in enumerate(source, start=1):
            try:
                row = json.loads(raw_line)
            except (json.JSONDecodeError, UnicodeDecodeError):
                print(f"warning: {path}:{line_number}: preserving malformed sidecar row", file=sys.stderr)
                continue
            if not isinstance(row, dict):
                continue
            tool_call_id, timestamp = row.get("toolCallId"), row.get("ts")
            if isinstance(tool_call_id, str) and isinstance(timestamp, str):
                keys.add((tool_call_id, timestamp))
    return keys


def append_sidecar_records(
    sidecar_path: Path,
    records: list[JSON],
    dry_run: bool,
) -> None:
    if dry_run or not records:
        return
    known = existing_sidecar_keys(sidecar_path)
    fresh = [
        record
        for record in records
        if (record["toolCallId"], record["ts"]) not in known
    ]
    if not fresh:
        return
    sidecar_path.parent.mkdir(parents=True, exist_ok=True)
    with sidecar_path.open("a+b") as output:
        # Preserve torn bytes, but never join a new record onto the old tail.
        if output.tell():
            output.seek(-1, os.SEEK_END)
            if output.read(1) != b"\n":
                output.write(b"\n")
        for record in fresh:
            encoded = json.dumps(record, ensure_ascii=False, separators=(",", ":"))
            output.write((encoded + "\n").encode("utf-8"))
        output.flush()
        os.fsync(output.fileno())


def message_container(row: Any) -> JSON | None:
    if not isinstance(row, dict):
        return None
    message = row.get("message")
    return message if isinstance(message, dict) else None


def migrate_file(path: Path, blobs_dir: Path, now: dt.datetime, dry_run: bool) -> FileStats:
    before = path.stat().st_size
    stats = FileStats(before=before, after=before)
    display_records: list[JSON] = []
    temporary: Path | None = None
    fd: int | None = None
    if not dry_run:
        fd, raw_temporary = tempfile.mkstemp(prefix=f".{path.name}.", dir=path.parent)
        temporary = Path(raw_temporary)

    try:
        output = (
            os.fdopen(fd, "w", encoding="utf-8")
            if fd is not None
            else None
        )
        with path.open(encoding="utf-8") as source:
            for line_number, raw_line in enumerate(source, start=1):
                try:
                    row = json.loads(raw_line)
                except json.JSONDecodeError as error:
                    raise RuntimeError(f"{path}:{line_number}: invalid JSON: {error}") from error
                message = message_container(row)
                if message is not None:
                    replacement, image_count = convert_content_images(
                        message.get("content"), blobs_dir, dry_run
                    )
                    if image_count:
                        message["content"] = replacement
                        stats.images += image_count
                    if main_transcript_path(path) and "tool_display" in message:
                        display = message.pop("tool_display")
                        tool_call_id = message.get("tool_call_id")
                        timestamp = row.get("timestamp")
                        if isinstance(tool_call_id, str) and isinstance(timestamp, str):
                            if expired_timestamp(timestamp, now):
                                display = expired_display_summary(display)
                            display_records.append(
                                {
                                    "toolCallId": tool_call_id,
                                    "ts": timestamp,
                                    "display": display,
                                }
                            )
                            stats.displays += 1
                        else:
                            # Do not lose a malformed display: leave it inline and make the
                            # operator repair the row rather than silently dropping data.
                            message["tool_display"] = display
                            print(
                                f"warning: {path}:{line_number}: tool_display lacks tool_call_id or timestamp",
                                file=sys.stderr,
                            )
                encoded = json.dumps(row, ensure_ascii=False, separators=(",", ":"))
                if output is not None:
                    output.write(encoded)
                    output.write("\n")
                stats.after += len(encoded.encode("utf-8")) + 1 - len(raw_line.encode("utf-8"))
        if output is not None:
            output.flush()
            os.fsync(output.fileno())
            output.close()
        # Sidecar first, transcript second: a crash can leave harmless display data, never a
        # transcript tool result whose display points to a missing sidecar row.
        append_sidecar_records(tool_display_sidecar_path(path), display_records, dry_run)
        if temporary is not None:
            os.replace(temporary, path)
            temporary = None
            # The transcript's size/mtime changed. Let the Rust reader rebuild this
            # derivative lazily instead of retaining stale byte offsets.
            resume_index_path(path).unlink(missing_ok=True)
    finally:
        if temporary is not None and temporary.exists():
            temporary.unlink()

    if not dry_run:
        stats.after = path.stat().st_size
    return stats


def backup_sessions(sessions_dir: Path) -> Path:
    timestamp = dt.datetime.now(dt.timezone.utc).strftime("%Y%m%dT%H%M%S%fZ")
    backup = sessions_dir / f".pre-migrate-{timestamp}"
    backup.mkdir()
    for child in sessions_dir.iterdir():
        if child == backup or child.name.startswith(".pre-migrate-"):
            continue
        target = backup / child.name
        if child.is_dir():
            shutil.copytree(child, target)
        else:
            shutil.copy2(child, target)
    return backup


def migrate(sessions_dir: Path, dry_run: bool, check_serve: bool = True) -> int:
    # Rust's integration fixture owns an isolated temporary sessions directory. Its test
    # subprocess sets this deliberately test-named escape hatch so unrelated local
    # `tomcat serve --stdio` processes cannot make the suite nondeterministic. The public
    # command has no flag for this: normal migrations always retain the race-prevention check.
    allow_running_serve_for_test = os.environ.get(TEST_ALLOW_RUNNING_SERVE_ENV) == "1"
    if check_serve and not allow_running_serve_for_test and is_serve_running():
        raise RuntimeError(
            "tomcat serve is running; close VS Code/Tomcat and retry so no transcript append races migration"
        )
    if not sessions_dir.is_dir():
        raise RuntimeError(f"sessions directory does not exist: {sessions_dir}")
    paths = list(session_paths(sessions_dir))
    if not dry_run:
        backup = backup_sessions(sessions_dir)
        print(f"backup: {backup}")

    now = dt.datetime.now(dt.timezone.utc)
    blobs_dir = sessions_dir / "attachments" / "blobs"
    total_before = total_after = image_count = display_count = 0
    for path in paths:
        stats = migrate_file(path, blobs_dir, now, dry_run)
        total_before += stats.before
        total_after += stats.after
        image_count += stats.images
        display_count += stats.displays
        print(
            f"{path.name}: {stats.before} -> {stats.after} bytes; "
            f"images={stats.images}; displays={stats.displays}"
        )
    print(
        f"total: {total_before} -> {total_after} bytes; "
        f"images={image_count}; displays={display_count}; dry_run={dry_run}"
    )
    if not dry_run:
        print(
            "next: start Tomcat once to run asynchronous attachment cleanup; "
            "unreferenced blobs remain for the one-hour crash-recovery grace period"
        )
    return 0


def self_test() -> int:
    root = Path(tempfile.mkdtemp(prefix="tomcat-transcript-v2-self-test-"))
    sessions = root / "sessions"
    sessions.mkdir()
    transcript = sessions / "sample.jsonl"
    legacy_backup = sessions / ".pre-migrate-existing"
    legacy_backup.mkdir()
    (legacy_backup / "must-not-nest.jsonl").write_text("legacy\n", encoding="utf-8")
    old_timestamp = "2020-01-01T00:00:00.000Z"
    encoded = base64.b64encode(b"image bytes").decode("ascii")
    rows = [
        {"type": "session", "id": "sample", "timestamp": old_timestamp},
        {
            "type": "message",
            "id": "tool-row",
            "timestamp": old_timestamp,
            "message": {
                "role": "tool",
                "tool_call_id": "call-1",
                "content": [
                    {
                        "type": "input_image",
                        "mime_type": "image/png",
                        "image_b64": encoded,
                    }
                ],
                "tool_display": {
                    "kind": "file",
                    "file": "src/demo.rs",
                    "added": 1,
                    "removed": 0,
                    "diff": [{"tag": "add", "text": "fn main() {}", "newLine": 1}],
                },
            },
        },
    ]
    transcript.write_text(
        "".join(json.dumps(row, separators=(",", ":")) + "\n" for row in rows),
        encoding="utf-8",
    )
    before_dry_run = transcript.read_bytes()
    migrate(sessions, dry_run=True, check_serve=False)
    assert transcript.read_bytes() == before_dry_run
    assert not (sessions / "attachments" / "blobs").exists()
    assert not tool_display_sidecar_path(transcript).exists()
    backup = backup_sessions(sessions)
    assert not (backup / legacy_backup.name).exists()
    resume_index_path(transcript).write_text("stale offsets", encoding="utf-8")
    migrate(sessions, dry_run=False, check_serve=False)
    once = transcript.read_bytes()
    migrated = [json.loads(line) for line in transcript.read_text(encoding="utf-8").splitlines()]
    message = migrated[1]["message"]
    assert message["content"][0]["type"] == "input_image_ref"
    assert "tool_display" not in message
    sha = message["content"][0]["blob_sha"]
    assert (sessions / "attachments" / "blobs" / sha).read_bytes() == b"image bytes"
    assert not resume_index_path(transcript).exists()
    sidecar = tool_display_sidecar_path(transcript)
    sidecar_rows = [json.loads(line) for line in sidecar.read_text(encoding="utf-8").splitlines()]
    assert len(sidecar_rows) == 1
    assert sidecar_rows[0]["display"]["expired"] is True
    assert sidecar_rows[0]["display"]["diff"] is None
    migrate(sessions, dry_run=False, check_serve=False)
    assert transcript.read_bytes() == once
    assert len(sidecar.read_text(encoding="utf-8").splitlines()) == 1
    prior_record = {
        "toolCallId": "prior",
        "ts": old_timestamp,
        "display": rows[1]["message"]["tool_display"],
    }
    prior_bytes = json.dumps(prior_record, ensure_ascii=False).encode("utf-8")
    for label, tail in [
        ("partial-json", b'{"toolCallId":"interrupted"'),
        ("partial-utf8", b'{"toolCallId":"interrupted","display":"\xe5'),
        ("complete-without-newline", prior_bytes),
    ]:
        case = root / label
        case.mkdir()
        source = case / "session.jsonl"
        source.write_bytes(before_dry_run)
        display_path = tool_display_sidecar_path(source)
        previous = prior_bytes + b"\n" + tail
        display_path.write_bytes(previous)
        blobs = case / "attachments" / "blobs"
        now = dt.datetime.now(dt.timezone.utc)
        migrate_file(source, blobs, now, dry_run=True)
        assert source.read_bytes() == before_dry_run, label
        assert display_path.read_bytes() == previous, label
        assert not blobs.exists(), label
        migrate_file(source, blobs, now, dry_run=False)
        published = source.read_bytes()
        sidecar_once = display_path.read_bytes()
        assert sidecar_once.startswith(previous), label
        readable = []
        for line in sidecar_once.split(b"\n"):
            try:
                readable.append(json.loads(line))
            except (json.JSONDecodeError, UnicodeDecodeError):
                pass  # Match the Rust reader's per-row malformed-data boundary.
        assert prior_record in readable, label
        assert sum(row["toolCallId"] == "call-1" for row in readable) == 1, label
        assert "tool_display" not in json.loads(published.splitlines()[1])["message"], label
        migrate_file(source, blobs, now, dry_run=False)
        assert source.read_bytes() == published, label
        assert display_path.read_bytes() == sidecar_once, label
        print(f"tail recovery passed: {label}")
    print(f"self-test passed: {root}")
    return 0


def run() -> int:
    args = parse_args()
    if args.self_test:
        return self_test()
    return migrate(args.sessions_dir.expanduser(), args.dry_run)


if __name__ == "__main__":
    try:
        raise SystemExit(run())
    except (OSError, RuntimeError) as error:
        print(f"error: {error}", file=sys.stderr)
        raise SystemExit(1)
