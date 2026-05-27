#!/usr/bin/env bash
set -euo pipefail

# Verify selected OpenAI endpoints using credentials/proxy from .env.
# Usage:
#   ./scripts/verify-openai-apis.sh              # interactive select
#   ./scripts/verify-openai-apis.sh 1 2 3 4      # non-interactive select
#
# Models: POST tests (2–5) use gpt-5.4 unless OPENAI_MODEL is set in .env.

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ENV_FILE="${ROOT_DIR}/.env"

if [[ ! -f "${ENV_FILE}" ]]; then
  echo "ERROR: .env not found at ${ENV_FILE}"
  exit 1
fi

set -a
# shellcheck disable=SC1090
source "${ENV_FILE}"
set +a

if [[ -z "${OPENAI_API_KEY:-}" ]]; then
  echo "ERROR: OPENAI_API_KEY is missing in .env"
  exit 1
fi

if [[ -n "${HTTPS_PROXY:-}" ]]; then
  echo "HTTPS_PROXY=${HTTPS_PROXY}"
else
  echo "HTTPS_PROXY=<empty> (direct connection)"
fi

key_prefix="${OPENAI_API_KEY:0:6}"
key_suffix="${OPENAI_API_KEY: -4}"
echo "OPENAI_API_KEY=${key_prefix}...${key_suffix} (masked)"

if [[ -n "${OPENAI_MODEL:-}" ]]; then
  MODELS=("${OPENAI_MODEL}")
else
  MODELS=(gpt-5.4)
fi

HAS_JQ=0
if command -v jq >/dev/null 2>&1; then
  HAS_JQ=1
fi

echo "Models for POST tests: ${MODELS[*]} (set OPENAI_MODEL in .env to use a single model)"

echo
echo "Available endpoints:"
echo "1) GET  /v1/models"
echo "2) POST /v1/responses"
echo "3) POST /v1/chat/completions"
echo "4) POST /v1/chat/completions (stream, SSE)"
echo "5) POST /v1/responses (stream, SSE)"

choices=()
if [[ "$#" -gt 0 ]]; then
  choices=("$@")
else
  read -r -p "Select endpoints (e.g. '1 2 3'): " input
  # shellcheck disable=SC2206
  choices=(${input})
fi

if [[ "${#choices[@]}" -eq 0 ]]; then
  echo "ERROR: no endpoint selected"
  exit 1
fi

pass_count=0
fail_count=0

print_fail_hint() {
  local msg="$1"
  local code="$2"

  if [[ "${code}" == "401" || "${msg}" == *"401"* ]]; then
    echo "Hint: 401 -> API key invalid/expired or not loaded."
  elif [[ "${code}" == "403" || "${msg}" == *"403"* ]]; then
    echo "Hint: 403 -> project/model permission issue."
  elif [[ "${code}" == "407" || "${msg}" == *"proxy"* || "${msg}" == *"CONNECT"* ]]; then
    echo "Hint: 407/CONNECT -> proxy unavailable or auth failed."
  elif [[ "${code}" == "429" || "${msg}" == *"429"* ]]; then
    echo "Hint: 429 -> rate limit or quota exhausted."
  elif [[ "${code}" == "500" || "${code}" == "502" || "${code}" == "503" || "${code}" == "504" ]]; then
    echo "Hint: 5xx -> upstream temporary error, retry later."
  else
    echo "Hint: check network/proxy/DNS and API base URL."
  fi
}

run_request() {
  local label="$1"
  local method="$2"
  local url="$3"
  local body="${4:-}"

  local tmp_body
  tmp_body="$(mktemp)"
  local http_code="000"
  local curl_exit=0

  if [[ -n "${body}" ]]; then
    http_code="$(curl -sS \
      --connect-timeout 10 --max-time 90 \
      -o "${tmp_body}" -w "%{http_code}" \
      -X "${method}" "${url}" \
      -H "Authorization: Bearer ${OPENAI_API_KEY}" \
      -H "Content-Type: application/json" \
      -d "${body}")" || curl_exit=$?
  else
    http_code="$(curl -sS \
      --connect-timeout 10 --max-time 90 \
      -o "${tmp_body}" -w "%{http_code}" \
      -X "${method}" "${url}" \
      -H "Authorization: Bearer ${OPENAI_API_KEY}" \
      -H "Content-Type: application/json")" || curl_exit=$?
  fi

  if [[ "${curl_exit}" -ne 0 ]]; then
    echo "[FAIL] ${label} - curl exit ${curl_exit}"
    print_fail_hint "curl_failed" "000"
    rm -f "${tmp_body}"
    fail_count=$((fail_count + 1))
    return
  fi

  if [[ "${http_code}" =~ ^2 ]]; then
    echo "[PASS] ${label} - HTTP ${http_code}"
    if [[ "${HAS_JQ}" -eq 1 ]]; then
      case "${label}" in
        *"GET /v1/models"*)
          jq -r '.data[0:5][]?.id' "${tmp_body}" | sed 's/^/  - /'
          ;;
        *"POST /v1/responses"*)
          jq '{id, model, output_text}' "${tmp_body}"
          ;;
        *"POST /v1/chat/completions"*)
          jq '{id, model, first: .choices[0].message.content}' "${tmp_body}"
          ;;
      esac
    else
      echo "  (jq not found; raw response omitted)"
    fi
    pass_count=$((pass_count + 1))
  else
    echo "[FAIL] ${label} - HTTP ${http_code}"
    local err_preview
    err_preview="$(tr -d '\n' < "${tmp_body}" | cut -c1-300)"
    echo "  Error: ${err_preview}"
    print_fail_hint "${err_preview}" "${http_code}"
    fail_count=$((fail_count + 1))
  fi

  rm -f "${tmp_body}"
}

# Streaming endpoints return text/event-stream (SSE). Validate HTTP 2xx and at least one `data:` line.
run_sse_stream_request() {
  local label="$1"
  local url="$2"
  local body="$3"

  local tmp_body
  tmp_body="$(mktemp)"
  local http_code="000"
  local curl_exit=0

  http_code="$(curl -sS \
    --connect-timeout 10 --max-time 180 \
    -o "${tmp_body}" -w "%{http_code}" \
    -X POST "${url}" \
    -H "Authorization: Bearer ${OPENAI_API_KEY}" \
    -H "Content-Type: application/json" \
    -H "Accept: text/event-stream" \
    -d "${body}")" || curl_exit=$?

  if [[ "${curl_exit}" -ne 0 ]]; then
    echo "[FAIL] ${label} - curl exit ${curl_exit}"
    print_fail_hint "curl_failed" "000"
    rm -f "${tmp_body}"
    fail_count=$((fail_count + 1))
    return
  fi

  local data_lines=0
  if [[ -s "${tmp_body}" ]]; then
    data_lines="$(grep -c '^data: ' "${tmp_body}" 2>/dev/null || true)"
  fi
  [[ -z "${data_lines}" ]] && data_lines=0

  if [[ "${http_code}" =~ ^2 ]]; then
    if [[ "${data_lines}" -lt 1 ]]; then
      echo "[FAIL] ${label} - HTTP ${http_code} but no SSE data: lines (got ${data_lines})"
      local err_preview
      err_preview="$(tr -d '\n' < "${tmp_body}" | cut -c1-300)"
      echo "  Body preview: ${err_preview}"
      print_fail_hint "${err_preview}" "${http_code}"
      fail_count=$((fail_count + 1))
      rm -f "${tmp_body}"
      return
    fi
    echo "[PASS] ${label} - HTTP ${http_code} (${data_lines} SSE data: lines)"
    if [[ "${HAS_JQ}" -eq 1 ]]; then
      case "${label}" in
        *"chat/completions"*)
          {
            local merged=""
            while IFS= read -r line || [[ -n "${line}" ]]; do
              [[ "${line}" == data:\ * ]] || continue
              local payload="${line#data: }"
              [[ "${payload}" == "[DONE]" ]] && continue
              local piece
              piece="$(echo "${payload}" | jq -r '.choices[0].delta.content // empty' 2>/dev/null || true)"
              merged="${merged}${piece}"
            done < "${tmp_body}"
            if [[ -n "${merged}" ]]; then
              echo "  merged delta (truncated): $(echo -n "${merged}" | cut -c1-200)$([[ ${#merged} -gt 200 ]] && echo -n "...")"
            else
              echo "  (no text in choices[].delta.content; stream shape may differ for this model)"
            fi
          }
          ;;
        *)
          echo "  (responses stream: showing first non-[DONE] data line, truncated)"
          local sample
          sample="$(grep '^data: ' "${tmp_body}" | grep -v 'data: \[DONE\]' | head -n 1 | sed 's/^data: //' | cut -c1-240)"
          [[ -n "${sample}" ]] && echo "  ${sample}..."
          ;;
      esac
    else
      echo "  (jq not found; SSE preview omitted)"
    fi
    pass_count=$((pass_count + 1))
  else
    echo "[FAIL] ${label} - HTTP ${http_code}"
    local err_preview
    err_preview="$(tr -d '\n' < "${tmp_body}" | cut -c1-300)"
    echo "  Error: ${err_preview}"
    print_fail_hint "${err_preview}" "${http_code}"
    fail_count=$((fail_count + 1))
  fi

  rm -f "${tmp_body}"
}

for c in "${choices[@]}"; do
  case "${c}" in
    1)
      run_request "GET /v1/models" "GET" "https://api.openai.com/v1/models"
      ;;
    2)
      for model in "${MODELS[@]}"; do
        run_request "POST /v1/responses [${model}]" "POST" "https://api.openai.com/v1/responses" \
          "{\"model\":\"${model}\",\"input\":\"Reply with: pong\"}"
      done
      ;;
    3)
      for model in "${MODELS[@]}"; do
        run_request "POST /v1/chat/completions [${model}]" "POST" "https://api.openai.com/v1/chat/completions" \
          "{\"model\":\"${model}\",\"messages\":[{\"role\":\"user\",\"content\":\"Reply with: pong\"}]}"
      done
      ;;
    4)
      for model in "${MODELS[@]}"; do
        run_sse_stream_request "POST /v1/chat/completions (stream) [${model}]" \
          "https://api.openai.com/v1/chat/completions" \
          "{\"model\":\"${model}\",\"messages\":[{\"role\":\"user\",\"content\":\"Reply with one word: hi\"}],\"stream\":true}"
      done
      ;;
    5)
      for model in "${MODELS[@]}"; do
        run_sse_stream_request "POST /v1/responses (stream) [${model}]" \
          "https://api.openai.com/v1/responses" \
          "{\"model\":\"${model}\",\"input\":\"Reply with one word: hi\",\"stream\":true}"
      done
      ;;
    *)
      echo "[SKIP] Unknown choice: ${c}"
      ;;
  esac
done

echo
echo "Summary: PASS=${pass_count} FAIL=${fail_count}"
if [[ "${fail_count}" -gt 0 ]]; then
  exit 2
fi
