#!/usr/bin/env python3
"""Summarize the five prompt-cache baseline metrics from one captured session.

Capture one real session with at least 20 model turns, then pass the combined
`tomcat_chat_diag` log and (when available) the serve event JSONL:

  python3 scripts/analyze_prompt_cache_baseline.py \
      --diag /tmp/tomcat.log --events /tmp/serve-events.jsonl --json baseline.json

The tool deliberately does no model replay. It reports only observed traffic:
cache read rate, session grants, plugin/skill catalog changes, rejected tool
calls, and end-of-turn context-watermark P50/P90. A capture below --min-turns
is rejected by default so before/after samples remain comparable.
"""

from __future__ import annotations

import argparse
import json
import math
import re
import sys
from dataclasses import asdict, dataclass, field
from pathlib import Path
from typing import Any, Iterable


FIELD_RE = re.compile(r"\b([A-Za-z_][A-Za-z0-9_]*)=([^\s]+)")


def percentile(values: list[float], fraction: float) -> float | None:
    if not values:
        return None
    ordered = sorted(values)
    index = (len(ordered) - 1) * fraction
    lower, upper = math.floor(index), math.ceil(index)
    if lower == upper:
        return ordered[lower]
    return ordered[lower] + (ordered[upper] - ordered[lower]) * (index - lower)


def fields(line: str) -> dict[str, str]:
    return {key: value.strip('"') for key, value in FIELD_RE.findall(line)}


def nested_event(record: dict[str, Any]) -> dict[str, Any]:
    """Return common wire envelopes without assuming one transport shape."""
    for key in ("event", "wire_event", "payload"):
        nested = record.get(key)
        if isinstance(nested, dict) and (
            isinstance(nested.get("type"), str) or isinstance(nested.get("kind"), str)
        ):
            return nested
    return record


def event_name(event: dict[str, Any]) -> str:
    return str(event.get("type") or event.get("kind") or "").lower()


@dataclass
class Baseline:
    model_turns: int = 0
    prompt_tokens: int = 0
    cache_read_tokens: int = 0
    cache_write_tokens: int = 0
    session_grants: int = 0
    plugin_or_skill_changes: int = 0
    rejected_tool_calls: int = 0
    total_tool_calls: int = 0
    context_utilization_ratios: list[float] = field(default_factory=list)

    def report(self) -> dict[str, Any]:
        hit_rate = (
            self.cache_read_tokens / self.prompt_tokens if self.prompt_tokens else None
        )
        rejection_rate = (
            self.rejected_tool_calls / self.total_tool_calls
            if self.total_tool_calls
            else None
        )
        return {
            "model_turns": self.model_turns,
            "cache": {
                "prompt_tokens": self.prompt_tokens,
                "cache_read_tokens": self.cache_read_tokens,
                "cache_write_tokens": self.cache_write_tokens,
                "hit_rate": hit_rate,
            },
            "session_grants": self.session_grants,
            "plugin_or_skill_changes": self.plugin_or_skill_changes,
            "tool_calls": {
                "total": self.total_tool_calls,
                "rejected": self.rejected_tool_calls,
                "rejection_rate": rejection_rate,
            },
            "turn_end_context_utilization": {
                "samples": len(self.context_utilization_ratios),
                "p50": percentile(self.context_utilization_ratios, 0.50),
                "p90": percentile(self.context_utilization_ratios, 0.90),
            },
        }


def read_diag(path: Path, baseline: Baseline) -> None:
    previous_catalog: tuple[str, str] | None = None
    with path.open(encoding="utf-8") as handle:
        for line in handle:
            parsed = fields(line)
            phase = parsed.get("phase")
            if phase == "llm_usage":
                baseline.model_turns += 1
                prompt_tokens = int(parsed.get("prompt_tokens", "0"))
                cache_read_tokens = int(parsed.get("cache_read_tokens", "0") or 0)
                baseline.prompt_tokens += prompt_tokens
                baseline.cache_read_tokens += cache_read_tokens
                baseline.cache_write_tokens += int(
                    parsed.get("cache_write_tokens", "0") or 0
                )
            elif phase == "session_grant_added":
                baseline.session_grants += 1
            elif phase == "prompt_runtime_snapshot":
                catalog = (
                    parsed.get("plugin_tools", ""),
                    parsed.get("visible_skills", ""),
                )
                if previous_catalog is not None and catalog != previous_catalog:
                    baseline.plugin_or_skill_changes += 1
                previous_catalog = catalog


def walk_records(path: Path) -> Iterable[dict[str, Any]]:
    with path.open(encoding="utf-8") as handle:
        for line in handle:
            try:
                value = json.loads(line)
            except json.JSONDecodeError:
                continue
            if isinstance(value, dict):
                yield nested_event(value)


def read_events(path: Path, baseline: Baseline) -> None:
    for event in walk_records(path):
        name = event_name(event)
        if name in {"tool_execution_end", "toolexecutionend"}:
            baseline.total_tool_calls += 1
            if event.get("isError", event.get("is_error")) is True:
                baseline.rejected_tool_calls += 1
        elif name in {"context_metrics_update", "contextmetricsupdate"}:
            value = event.get("context_utilization_ratio")
            if value is None:
                value = event.get("contextUtilizationRatio")
            if isinstance(value, (int, float)):
                baseline.context_utilization_ratios.append(float(value))


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--diag", type=Path, required=True, help="tomcat_chat_diag log")
    parser.add_argument("--events", type=Path, help="optional serve event JSONL")
    parser.add_argument("--json", type=Path, help="write report to this path")
    parser.add_argument("--min-turns", type=int, default=20)
    parser.add_argument("--allow-short", action="store_true")
    args = parser.parse_args()

    baseline = Baseline()
    read_diag(args.diag, baseline)
    if args.events:
        read_events(args.events, baseline)

    if baseline.model_turns < args.min_turns and not args.allow_short:
        print(
            f"need at least {args.min_turns} model turns; observed {baseline.model_turns}",
            file=sys.stderr,
        )
        return 2

    report = baseline.report()
    rendered = json.dumps(report, ensure_ascii=False, indent=2) + "\n"
    if args.json:
        args.json.write_text(rendered, encoding="utf-8")
    else:
        sys.stdout.write(rendered)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
