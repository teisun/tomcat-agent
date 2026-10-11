#!/usr/bin/env python3
"""Replay one transcript user turn against an OpenAI Responses endpoint.

This is a debugging helper for attachment/request-shape incidents. It rebuilds a
`POST /v1/responses` body from a transcript using the same high-level rules as
`src/core/llm/openai_responses/payload.rs`:

- first system message -> top-level `instructions`
- user multimodal parts -> `input_text` + `input_image` / `input_file`
- assistant text -> `output_text`
- assistant tool calls -> `function_call`
- tool messages -> `function_call_output`
- compatible reasoning continuity -> replay opaque reasoning items
- incompatible continuity -> downgrade to visible text only

By default the script prints a sanitized request/response summary and never
prints inline bytes. It is intentionally focused on replaying the request shape
that matters for attachment and stream-terminal-error debugging.
Stored attachment references (input_image_ref/blob_sha) are not rehydrated by
this helper; they are rejected instead of silently replaying a text-only request.

Examples:
  python3 scripts/replay-turn-payload.py \
    ~/.tomcat/agents/main/sessions/1785033466207_edbc439326b469cf.jsonl \
    --turn-number 2 \
    --model gpt-5.6-sol \
    --provider fcodex \
    --base-url https://fcodex.top \
    --api-key-env OPENAI_API_KEY \
    --env-file .env
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import sys
import urllib.error
import urllib.request
from dataclasses import dataclass
from pathlib import Path
from typing import Any


def load_dotenv(path: Path) -> None:
    if not path.is_file():
        return
    for raw in path.read_text(encoding="utf-8").splitlines():
        line = raw.strip()
        if not line or line.startswith("#") or "=" not in line:
            continue
        key, value = line.split("=", 1)
        key = key.strip()
        value = value.strip().strip("'").strip('"')
        os.environ.setdefault(key, value)


def model_family(model: str) -> str:
    lower = model.strip().lower()
    if lower.startswith("deepseek-v4-pro") or lower.startswith("deepseek-v4-flash"):
        return "deepseek-v4"
    if lower.startswith("gpt-5"):
        return "gpt-5"
    if lower.startswith("claude-opus-4-"):
        return "claude-opus-4"
    return lower or "unknown"


def normalize_route_component(raw: str, fallback: str) -> str:
    trimmed = raw.strip().rstrip("/").lower()
    return trimmed or fallback


def credential_fingerprint(api_key: str) -> str:
    return hashlib.sha256(api_key.encode("utf-8")).hexdigest()[:16]


def routed_profile_id(provider: str, model: str, base_url: str, api_key: str) -> str:
    return "openai.responses.route/{provider}/{family}/{base}/{fingerprint}".format(
        provider=normalize_route_component(provider, "openai"),
        family=model_family(model),
        base=normalize_route_component(base_url, "default-base"),
        fingerprint=normalize_route_component(
            credential_fingerprint(api_key), "anonymous-credential"
        ),
    )


def response_endpoint(base_url: str) -> str:
    trimmed = base_url.rstrip("/")
    if trimmed.endswith("/v1/responses"):
        return trimmed
    if trimmed.endswith("/v1"):
        return f"{trimmed}/responses"
    return f"{trimmed}/v1/responses"


def read_transcript_messages(path: Path) -> list[dict[str, Any]]:
    messages: list[dict[str, Any]] = []
    with path.open("r", encoding="utf-8") as handle:
        for idx, raw in enumerate(handle, start=1):
            line = raw.strip()
            if not line:
                continue
            try:
                entry = json.loads(line)
            except json.JSONDecodeError as exc:
                raise SystemExit(f"line {idx}: invalid JSON: {exc}") from exc
            if entry.get("type") != "message":
                continue
            message = entry.get("message")
            if not isinstance(message, dict):
                continue
            role = infer_role(message)
            messages.append(
                {
                    "entry_id": entry.get("id"),
                    "timestamp": entry.get("timestamp"),
                    "role": role,
                    "kind": message.get("kind") or "normal",
                    "message": message,
                }
            )
    if not messages:
        raise SystemExit(f"no transcript message rows found in {path}")
    return messages


def infer_role(message: dict[str, Any]) -> str:
    role = message.get("role")
    if isinstance(role, str) and role:
        return role
    if "tool_call_id" in message:
        return "tool"
    return "assistant"


def select_turn(
    messages: list[dict[str, Any]],
    *,
    turn_index: int | None,
    turn_number: int | None,
    message_id: str | None,
) -> tuple[int, dict[str, Any]]:
    if sum(value is not None for value in (turn_index, turn_number, message_id)) != 1:
        raise SystemExit("choose exactly one of --turn-index, --turn-number, or --message-id")

    user_indices = [
        idx
        for idx, item in enumerate(messages)
        if item["role"] == "user" and item.get("kind", "normal") == "normal"
    ]
    if not user_indices:
        raise SystemExit("transcript does not contain any normal user turns")

    if message_id is not None:
        for idx in user_indices:
            if messages[idx].get("entry_id") == message_id:
                return idx, messages[idx]
        raise SystemExit(f"user message id not found: {message_id}")

    if turn_number is not None:
        if turn_number <= 0:
            raise SystemExit("--turn-number is 1-based and must be >= 1")
        ordinal = turn_number - 1
    else:
        assert turn_index is not None
        ordinal = turn_index if turn_index >= 0 else len(user_indices) + turn_index

    if ordinal < 0 or ordinal >= len(user_indices):
        raise SystemExit(
            f"selected turn out of range: have {len(user_indices)} normal user turn(s)"
        )
    idx = user_indices[ordinal]
    return idx, messages[idx]


def same_profile(
    continuation: dict[str, Any], *, target_profile_id: str, target_model: str
) -> bool:
    refs = continuation.get("provider_refs") or {}
    replay_profile_id = refs.get("replay_profile_id")
    if isinstance(replay_profile_id, str) and replay_profile_id:
        return (
            continuation.get("source_api") == "responses"
            and model_family(str(continuation.get("source_model") or "")) == model_family(target_model)
            and replay_profile_id == target_profile_id
        )
    if continuation.get("source_api") == "responses" and target_profile_id != "openai.responses.default":
        return False
    return (
        continuation.get("source_provider") == "openai"
        and continuation.get("source_api") == "responses"
        and model_family(str(continuation.get("source_model") or "")) == model_family(target_model)
    )


def replay_window_contains(messages: list[dict[str, Any]], idx: int) -> bool:
    current_turn_start = 0
    for pos, item in enumerate(messages):
        if item["role"] == "user" and item.get("kind", "normal") == "normal":
            current_turn_start = pos + 1
    return idx >= current_turn_start


def extract_visible_text(content: Any) -> str:
    if content is None:
        return ""
    if isinstance(content, str):
        return content
    if not isinstance(content, list):
        return ""
    chunks: list[str] = []
    for part in content:
        if not isinstance(part, dict):
            continue
        if part.get("type") == "input_text":
            text = part.get("text")
            if isinstance(text, str):
                chunks.append(text)
            continue
        reference = part.get("reference")
        if isinstance(reference, str):
            chunks.append(reference)
    return "".join(chunks)


def response_reasoning_items(continuation: dict[str, Any]) -> list[dict[str, Any]]:
    if continuation.get("format") != "openai_responses_reasoning_items":
        return []
    payload = continuation.get("opaque_payload")
    if isinstance(payload, list):
        out: list[dict[str, Any]] = []
        for item in payload:
            if not isinstance(item, dict):
                continue
            kind = str(item.get("type") or "")
            if "reasoning" in kind or "encrypted_content" in item:
                out.append(item)
        return out
    if isinstance(payload, dict):
        return [payload]
    return []


def user_content_parts(content: Any) -> list[dict[str, Any]]:
    if isinstance(content, str):
        return [{"type": "input_text", "text": content}]
    if not isinstance(content, list):
        return [{"type": "input_text", "text": ""}]

    text_chunks: list[str] = []
    out: list[dict[str, Any]] = []
    for part in content:
        if not isinstance(part, dict):
            continue
        part_type = part.get("type")
        if part_type == "input_image_ref" or "blob_sha" in part:
            raise SystemExit(
                "Stored attachment references are unsupported by this replay helper; "
                "only inline media can be replayed."
            )
        if part_type == "input_text":
            text = part.get("text")
            if isinstance(text, str):
                text_chunks.append(text)
            continue
        if "reference" in part and isinstance(part["reference"], str):
            text_chunks.append(part["reference"])
            continue
        if "image_b64" in part:
            mime = str(part.get("mime_type") or "image/png")
            encoded = str(part["image_b64"])
            item: dict[str, Any] = {
                "type": "input_image",
                "image_url": f"data:{mime};base64,{encoded}",
            }
            detail = part.get("detail")
            if isinstance(detail, str) and detail:
                item["detail"] = detail
            out.append(item)
            continue
        if "file_b64" in part:
            mime = str(part.get("mime_type") or "application/octet-stream")
            filename = str(part.get("filename") or "attachment.bin")
            encoded = str(part["file_b64"])
            out.append(
                {
                    "type": "input_file",
                    "filename": filename,
                    "file_data": f"data:{mime};base64,{encoded}",
                }
            )
            continue
    out.insert(0, {"type": "input_text", "text": "".join(text_chunks)})
    return out or [{"type": "input_text", "text": ""}]


@dataclass
class BuildResult:
    body: dict[str, Any]
    selected_user_entry_id: str | None
    selected_user_timestamp: str | None


def build_request_body(
    messages: list[dict[str, Any]],
    *,
    model: str,
    stream: bool,
    include_reasoning_summary: bool,
    reasoning_effort: str | None,
    temperature: float | None,
    max_output_tokens: int | None,
    tools: list[dict[str, Any]] | None,
    continuity_enabled: bool,
    use_previous_response_id: bool,
    target_profile_id: str,
) -> BuildResult:
    instructions: str | None = None
    input_items: list[dict[str, Any]] = []
    first_seen = False
    previous_response_id: str | None = None

    for idx, item in enumerate(messages):
        message = item["message"]
        role = item["role"]
        content = message.get("content")
        continuation = message.get("reasoning_continuation")
        visible_text = extract_visible_text(content)
        in_window = replay_window_contains(messages, idx)
        keep_opaque = False

        if role == "assistant" and continuity_enabled and isinstance(continuation, dict):
            if in_window and same_profile(
                continuation,
                target_profile_id=target_profile_id,
                target_model=model,
            ):
                keep_opaque = True

        if role == "system":
            if not first_seen and instructions is None:
                instructions = visible_text
                first_seen = True
                continue
            first_seen = True
            input_items.append(
                {
                    "type": "message",
                    "role": "system",
                    "content": [{"type": "input_text", "text": visible_text}],
                }
            )
            continue

        if role == "user":
            first_seen = True
            input_items.append(
                {
                    "type": "message",
                    "role": "user",
                    "content": user_content_parts(content),
                }
            )
            continue

        if role == "assistant":
            first_seen = True
            if keep_opaque:
                input_items.extend(response_reasoning_items(continuation))
                refs = continuation.get("provider_refs") or {}
                candidate = refs.get("openai_response_id")
                if isinstance(candidate, str) and candidate and previous_response_id is None:
                    previous_response_id = candidate
            tool_calls = message.get("tool_calls") or []
            if not tool_calls:
                if visible_text:
                    input_items.append(
                        {
                            "type": "message",
                            "role": "assistant",
                            "content": [{"type": "output_text", "text": visible_text}],
                        }
                    )
                continue
            if visible_text:
                input_items.append(
                    {
                        "type": "message",
                        "role": "assistant",
                        "content": [{"type": "output_text", "text": visible_text}],
                    }
                )
            for tool_call in tool_calls:
                if not isinstance(tool_call, dict):
                    continue
                function = tool_call.get("function") or {}
                input_items.append(
                    {
                        "type": "function_call",
                        "call_id": str(tool_call.get("id") or ""),
                        "name": str(function.get("name") or ""),
                        "arguments": str(function.get("arguments") or ""),
                    }
                )
            continue

        if role == "tool":
            input_items.append(
                {
                    "type": "function_call_output",
                    "call_id": str(message.get("tool_call_id") or ""),
                    "output": visible_text,
                }
            )

    body: dict[str, Any] = {
        "model": model,
        "input": input_items,
        "stream": stream,
        "store": False,
    }

    explicit_replay = continuity_enabled
    if use_previous_response_id and previous_response_id:
        body["store"] = True
        body["previous_response_id"] = previous_response_id
        explicit_replay = False
    if explicit_replay:
        body["include"] = ["reasoning.encrypted_content"]
    if instructions is not None:
        body["instructions"] = instructions
    if temperature is not None:
        body["temperature"] = temperature
    if max_output_tokens is not None:
        body["max_output_tokens"] = max(16, max_output_tokens)
    if tools:
        body["tools"] = tools
    reasoning: dict[str, Any] = {}
    if reasoning_effort:
        reasoning["effort"] = reasoning_effort
    if include_reasoning_summary:
        reasoning["summary"] = "auto"
    if reasoning:
        body["reasoning"] = reasoning

    selected = messages[-1]
    return BuildResult(
        body=body,
        selected_user_entry_id=selected.get("entry_id"),
        selected_user_timestamp=selected.get("timestamp"),
    )


def sanitize_value(value: Any, key: str | None = None) -> Any:
    if isinstance(value, dict):
        return {k: sanitize_value(v, k) for k, v in value.items()}
    if isinstance(value, list):
        return [sanitize_value(item, key) for item in value]
    if not isinstance(value, str):
        return value

    if key in {"image_b64", "file_b64"}:
        return f"<redacted base64 chars={len(value)}>"
    if key in {"image_url", "file_data"} and value.startswith("data:") and ";base64," in value:
        prefix, encoded = value.split(",", 1)
        mime = prefix[5:].split(";", 1)[0]
        return f"<redacted data-uri mime={mime} base64_chars={len(encoded)}>"
    if key == "encrypted_content":
        return f"<redacted encrypted_content chars={len(value)}>"
    return value


def request_shape(body: dict[str, Any]) -> dict[str, Any]:
    kinds: list[str] = []
    for item in body.get("input", []):
        if not isinstance(item, dict):
            continue
        item_type = str(item.get("type") or "unknown")
        if item_type == "message":
            content = item.get("content") or []
            part_types = [
                str(part.get("type") or "unknown")
                for part in content
                if isinstance(part, dict)
            ]
            kinds.append(f"message:{item.get('role')}[{','.join(part_types)}]")
        else:
            kinds.append(item_type)
    return {
        "model": body.get("model"),
        "store": body.get("store"),
        "has_instructions": isinstance(body.get("instructions"), str),
        "include": body.get("include"),
        "previous_response_id": body.get("previous_response_id"),
        "input_items": len(body.get("input", [])),
        "shape": kinds,
    }


def parse_sse(raw: bytes) -> list[Any]:
    text = raw.decode("utf-8", errors="replace")
    events: list[Any] = []
    for block in text.split("\n\n"):
        lines = [line[6:] for line in block.splitlines() if line.startswith("data: ")]
        if not lines:
            continue
        payload = "\n".join(lines).strip()
        if not payload:
            continue
        if payload == "[DONE]":
            events.append({"type": "[DONE]"})
            continue
        try:
            events.append(json.loads(payload))
        except json.JSONDecodeError:
            events.append({"type": "unparsed", "raw": payload})
    return events


def summarize_terminal(events: list[Any]) -> dict[str, Any]:
    summary: dict[str, Any] = {
        "events": len(events),
        "types": {},
        "terminal_event": None,
    }
    for event in events:
        if isinstance(event, dict):
            event_type = str(event.get("type") or "unknown")
        else:
            event_type = type(event).__name__
        summary["types"][event_type] = summary["types"].get(event_type, 0) + 1
    for event in reversed(events):
        if isinstance(event, dict):
            event_type = str(event.get("type") or "")
            if event_type in {"response.completed", "response.failed", "response.incomplete", "error"}:
                summary["terminal_event"] = sanitize_value(event)
                break
    return summary


def post_request(
    endpoint: str,
    *,
    api_key: str,
    body: dict[str, Any],
    timeout_sec: float,
) -> dict[str, Any]:
    payload = json.dumps(body, ensure_ascii=False).encode("utf-8")
    request = urllib.request.Request(
        endpoint,
        data=payload,
        method="POST",
        headers={
            "Authorization": f"Bearer {api_key}",
            "Content-Type": "application/json",
            "Accept": "text/event-stream" if body.get("stream") else "application/json",
        },
    )
    try:
        with urllib.request.urlopen(request, timeout=timeout_sec) as response:
            raw = response.read()
            content_type = response.headers.get("Content-Type", "")
            status = response.status
    except urllib.error.HTTPError as exc:
        raw = exc.read()
        content_type = exc.headers.get("Content-Type", "")
        status = exc.code
    except urllib.error.URLError as exc:
        raise SystemExit(f"request failed: {exc}") from exc

    result: dict[str, Any] = {
        "status": status,
        "content_type": content_type,
    }
    if "text/event-stream" in content_type or raw.startswith(b"data: "):
        events = parse_sse(raw)
        result["stream_terminal"] = summarize_terminal(events)
    else:
        text = raw.decode("utf-8", errors="replace")
        try:
            result["json"] = sanitize_value(json.loads(text))
        except json.JSONDecodeError:
            result["text_preview"] = text[:1000]
    return result


def load_tools(path: Path | None) -> list[dict[str, Any]] | None:
    if path is None:
        return None
    raw = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(raw, list):
        raise SystemExit("--tools-json must point to a JSON array")
    return raw


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("transcript", type=Path, help="Path to transcript JSONL")
    parser.add_argument("--turn-index", type=int, help="0-based normal user turn index")
    parser.add_argument("--turn-number", type=int, help="1-based normal user turn number")
    parser.add_argument("--message-id", help="Replay up to this user message id")
    parser.add_argument("--model", required=True, help="Request model name")
    parser.add_argument("--provider", default="openai", help="Route provider label")
    parser.add_argument("--base-url", required=True, help="Base URL, e.g. https://fcodex.top")
    parser.add_argument(
        "--api-key-env",
        default="OPENAI_API_KEY",
        help="Environment variable holding the API key",
    )
    parser.add_argument(
        "--env-file",
        type=Path,
        default=Path(".env"),
        help="Optional dotenv file to preload before resolving the API key",
    )
    parser.add_argument("--timeout-sec", type=float, default=180.0)
    parser.add_argument("--temperature", type=float)
    parser.add_argument("--max-output-tokens", type=int)
    parser.add_argument("--reasoning-effort", choices=["minimal", "low", "medium", "high", "xhigh"])
    parser.add_argument(
        "--reasoning-summary",
        action="store_true",
        help="Include reasoning.summary=auto",
    )
    parser.add_argument(
        "--continuity-enabled",
        action="store_true",
        default=True,
        help="Keep transcript-first continuity logic enabled (default: on)",
    )
    parser.add_argument(
        "--no-continuity",
        dest="continuity_enabled",
        action="store_false",
        help="Disable continuity replay/include handling",
    )
    parser.add_argument(
        "--use-previous-response-id",
        action="store_true",
        help="Enable the store=true + previous_response_id fast path when available",
    )
    parser.add_argument(
        "--tools-json",
        type=Path,
        help="Optional path to a JSON array of Responses tools",
    )
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="Only print the sanitized request shape; do not send the request",
    )
    args = parser.parse_args()

    transcript = args.transcript.expanduser().resolve()
    if not transcript.is_file():
        raise SystemExit(f"transcript not found: {transcript}")

    env_file = args.env_file.expanduser().resolve()
    load_dotenv(env_file)
    api_key = os.environ.get(args.api_key_env, "").strip()
    if not api_key:
        raise SystemExit(
            f"{args.api_key_env} is empty; pass --env-file or export the API key first"
        )

    messages = read_transcript_messages(transcript)
    selected_idx, selected = select_turn(
        messages,
        turn_index=args.turn_index,
        turn_number=args.turn_number,
        message_id=args.message_id,
    )
    selected_messages = messages[: selected_idx + 1]
    target_profile_id = routed_profile_id(args.provider, args.model, args.base_url, api_key)

    result = build_request_body(
        selected_messages,
        model=args.model,
        stream=True,
        include_reasoning_summary=args.reasoning_summary,
        reasoning_effort=args.reasoning_effort,
        temperature=args.temperature,
        max_output_tokens=args.max_output_tokens,
        tools=load_tools(args.tools_json),
        continuity_enabled=args.continuity_enabled,
        use_previous_response_id=args.use_previous_response_id,
        target_profile_id=target_profile_id,
    )

    print(
        json.dumps(
            {
                "transcript": str(transcript),
                "selected_user_message_id": selected.get("entry_id"),
                "selected_user_timestamp": selected.get("timestamp"),
                "target_profile_id": target_profile_id,
                "request_shape": request_shape(result.body),
                "request_preview": sanitize_value(result.body),
            },
            ensure_ascii=False,
            indent=2,
        )
    )

    if args.dry_run:
        return

    endpoint = response_endpoint(args.base_url)
    replay = post_request(
        endpoint,
        api_key=api_key,
        body=result.body,
        timeout_sec=args.timeout_sec,
    )
    print(
        json.dumps(
            {
                "endpoint": endpoint,
                "result": replay,
            },
            ensure_ascii=False,
            indent=2,
        )
    )


if __name__ == "__main__":
    main()
