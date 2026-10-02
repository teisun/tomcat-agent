#!/usr/bin/env python3
"""Bounded OpenAI Responses tier probe. Dry-run by default; keys come only from env.

Standard, Fast, and declared Ultrafast all use non-streaming JSON with Max.
At most 18 requests, serial, no redirects/retries, 120-second total deadline each.
A successful request proves parameter acceptance, not actual reasoning intensity.
"""

import argparse
from contextlib import contextmanager
import json
import os
from pathlib import Path
import re
import signal
import time
import tomllib
from urllib.error import HTTPError, URLError
from urllib.parse import urlsplit
from urllib.request import HTTPRedirectHandler, Request, build_opener

TIERS = {"standard": "default", "fast": "priority", "ultrafast": "ultrafast"}
MAX_REQUESTS = 18
TIMEOUT = 120


class NoRedirect(HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None  # Never forward a credential to a redirected destination.


@contextmanager
def deadline():
    # POSIX timer interrupts blocking socket reads too: timeout= alone is idle time.
    if not hasattr(signal, "setitimer"):
        raise RuntimeError("Live probing requires POSIX total-deadline support")

    def expire(_signum, _frame):
        raise TimeoutError("request deadline")

    previous = signal.signal(signal.SIGALRM, expire)
    signal.setitimer(signal.ITIMER_REAL, TIMEOUT)
    try:
        yield
    finally:
        signal.setitimer(signal.ITIMER_REAL, 0)
        signal.signal(signal.SIGALRM, previous)


def responses_url(base):
    parsed = urlsplit(base.strip().rstrip("/"))
    if (parsed.scheme != "https" or not parsed.hostname or parsed.username
            or parsed.password or parsed.query or parsed.fragment):
        raise ValueError("Models must declare a credential-free HTTPS base URL")
    # Same path-aware rule as core/llm/endpoint.rs: explicit paths are preserved.
    return base.strip().rstrip("/") + ("/responses" if parsed.path not in ("", "/") else "/v1/responses")


def safe_usage(response):
    usage = response.get("usage") or {}
    if not isinstance(usage, dict):
        return {}
    result = {key: usage[key] for key in ("input_tokens", "output_tokens", "total_tokens")
              if isinstance(usage.get(key), int)}
    for key, detail in (("input_tokens_details", "cached_tokens"), ("output_tokens_details", "reasoning_tokens")):
        values = usage.get(key)
        if isinstance(values, dict) and isinstance(values.get(detail), int):
            result[key] = {detail: values[detail]}
    return result


def has_text(response):
    return any(bool(str(part.get("text") or "").strip())
               for output in (response.get("output") or []) if isinstance(output, dict)
               for part in (output.get("content") or []) if isinstance(part, dict))


def probe(model, speed, url):
    row = {"catalog_id": model["id"], "wire_model": model.get("model_name") or model["id"],
           "host": urlsplit(url).hostname, "key_env": model["api_key_env"],
           "requested_speed": speed, "requested_tier": TIERS[speed], "effort": "max",
           "http_status": None, "actual_tier": None, "status": None, "accepted": False,
           "tier_confirmed": False, "downgraded": False, "truncated": False,
           "text_nonempty": False, "usage": {}, "elapsed_seconds": 0,
           "outcome": "SKIP", "error_category": None, "error_code": None}
    key = os.environ.get(model["api_key_env"], "").strip()
    if not key:
        row["error_code"] = "missing_key"
        return row
    started = time.monotonic()
    body = {"model": row["wire_model"], "input": "Reply only OK.", "store": False,
            "reasoning": {"effort": "max"}, "service_tier": TIERS[speed],
            "max_output_tokens": 2048, "stream": False}
    request = Request(url, json.dumps(body).encode(), headers={
        "Authorization": "Bearer " + key, "Content-Type": "application/json",
        "Accept": "application/json"})
    try:
        with deadline(), build_opener(NoRedirect()).open(request, timeout=TIMEOUT) as reply:
            row["http_status"] = reply.status
            response = json.load(reply)
        if not isinstance(response, dict):
            raise ValueError("invalid_response")
        row["status"] = response.get("status") if response.get("status") in ("completed", "incomplete", "failed", "cancelled", "queued", "in_progress") else None
        row["accepted"] = 200 <= row["http_status"] < 300 and not response.get("error") and row["status"] != "failed"
        actual = response.get("service_tier")
        row["actual_tier"] = actual if actual in ("default", "priority", "fast", "ultrafast", "auto", "flex") else None
        row["tier_confirmed"] = row["accepted"] and (row["actual_tier"] == TIERS[speed] or speed == "fast" and row["actual_tier"] == "fast")
        row["downgraded"] = row["accepted"] and speed != "standard" and row["actual_tier"] == "default"
        row["truncated"] = row["status"] == "incomplete" and (response.get("incomplete_details") or {}).get("reason") == "max_output_tokens"
        row["text_nonempty"] = has_text(response)
        row["usage"] = safe_usage(response)
        row["outcome"] = "accepted_truncated" if row["accepted"] and row["truncated"] else "accepted" if row["accepted"] else "failed"
        if not row["accepted"]:
            row.update(error_category="upstream_response", error_code="upstream_error")
    except HTTPError as error:
        row.update(http_status=error.code, outcome="failed", error_category="upstream_http",
                   error_code="redirect_refused" if 300 <= error.code < 400 else f"http_{error.code}")
        error.close()  # Never print/read an upstream error body that might echo secrets.
    except TimeoutError:
        row.update(outcome="failed", error_category="transport", error_code="deadline_exceeded")
    except URLError:
        row.update(outcome="failed", error_category="transport", error_code="network_error")
    except (OSError, ValueError, TypeError, AttributeError):
        row.update(accepted=False, tier_confirmed=False, outcome="failed", error_category="response_or_transport", error_code="invalid_or_interrupted_response")
    finally:
        row["elapsed_seconds"] = round(time.monotonic() - started, 3)
    return row


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--models-file", type=Path, default=Path.home() / ".tomcat/models.toml")
    parser.add_argument("--model", action="append", default=[], help="Catalog ID (repeatable); default: GPT-5.6+ entries")
    parser.add_argument("--live", action="store_true", help="Send the bounded, potentially paid requests")
    parser.add_argument("--report", type=Path, help="New JSON report file (never overwrites)")
    args = parser.parse_args()
    if args.report and args.report.exists():
        parser.error("Report already exists; choose a new path before sending requests")
    with args.models_file.open("rb") as source:
        catalog = tomllib.load(source)["models"]
    by_id = {model["id"]: model for model in catalog}
    if args.model:
        if len(set(args.model)) != len(args.model) or any(model not in by_id for model in args.model):
            parser.error("Catalog IDs must exist and may not repeat")
        models = [by_id[model] for model in args.model]
    else:
        models = []
        for model in catalog:
            match = re.match(r"^gpt-(\d+)(?:\.(\d+))?(?:-|$)", model.get("model_name") or model["id"])
            if match and (int(match[1]), int(match[2] or 0)) >= (5, 6):
                models.append(model)
    calls = []
    for model in models:
        if model.get("api") != "openai-responses" or "max" not in model.get("supported_reasoning_levels", []):
            parser.error("Selected models must declare openai-responses and max")
        if not re.fullmatch(r"[A-Z_][A-Z0-9_]*", model.get("api_key_env", "")):
            parser.error("Selected models must declare a valid api_key_env")
        speeds = model.get("supported_speeds", [])
        if "fast" not in speeds or any(speed not in ("fast", "ultrafast") for speed in speeds):
            parser.error("Selected models must declare fast and optional ultrafast")
        url = responses_url(model["base_url"])
        calls.extend((model, speed, url) for speed in TIERS if speed == "standard" or speed in speeds)
    if not 0 < len(calls) <= MAX_REQUESTS:
        parser.error("Plan must contain 1–18 requests; narrow the --model selection")
    print(f"{'LIVE' if args.live else 'DRY-RUN (no network)'}: {len(models)} models, {len(calls)} planned requests, no retries")
    rows = []
    blocked = set()
    for model, speed, url in calls:
        if not args.live:
            row = {"catalog_id": model["id"], "wire_model": model.get("model_name") or model["id"],
                   "host": urlsplit(url).hostname, "key_env": model["api_key_env"],
                   "key_present": bool(os.environ.get(model["api_key_env"], "").strip()),
                   "requested_speed": speed, "requested_tier": TIERS[speed], "effort": "max",
                   "outcome": "dry_run"}
        elif model["id"] in blocked:
            row = {"catalog_id": model["id"], "wire_model": model.get("model_name") or model["id"],
                   "host": urlsplit(url).hostname, "key_env": model["api_key_env"],
                   "requested_speed": speed, "requested_tier": TIERS[speed], "effort": "max",
                   "outcome": "SKIP", "error_code": "previous_tier_failed"}
        else:
            row = probe(model, speed, url)
            if not row["accepted"]:
                blocked.add(model["id"])
        rows.append(row)
        print(json.dumps(row, ensure_ascii=False), flush=True)
    summary = {"mode": "live" if args.live else "dry_run", "planned": len(calls),
               "attempted": sum(row["outcome"] not in ("SKIP", "dry_run") for row in rows),
               "accepted": sum(row.get("accepted", False) for row in rows),
               "tier_confirmed": sum(row.get("tier_confirmed", False) for row in rows),
               "downgraded": sum(row.get("downgraded", False) for row in rows),
               "truncated": sum(row.get("truncated", False) for row in rows),
               "failed": sum(row["outcome"] == "failed" for row in rows),
               "skipped": sum(row["outcome"] == "SKIP" for row in rows)}
    print(json.dumps(summary), flush=True)
    if args.report:
        with args.report.open("x", encoding="utf-8") as output:
            json.dump({"summary": summary, "results": rows}, output, indent=2, ensure_ascii=False)
            output.write("\n")
    # SKIP or accepted-but-unconfirmed paid tiers are not a verified success.
    return 1 if args.live and (summary["failed"] or summary["skipped"] or any(not row.get("tier_confirmed") for row in rows)) else 0


if __name__ == "__main__":
    raise SystemExit(main())
