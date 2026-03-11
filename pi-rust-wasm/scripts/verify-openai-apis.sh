#!/usr/bin/env bash
set -euo pipefail

# Verify selected OpenAI endpoints using credentials/proxy from .env.
# Usage:
#   ./scripts/verify-openai-apis.sh           # interactive select
#   ./scripts/verify-openai-apis.sh 1 2 3     # non-interactive select

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

MODEL="${OPENAI_MODEL:-gpt-5.2}"
HAS_JQ=0
if command -v jq >/dev/null 2>&1; then
  HAS_JQ=1
fi

echo
echo "Available endpoints:"
echo "1) GET  /v1/models"
echo "2) POST /v1/responses"
echo "3) POST /v1/chat/completions"

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
        "GET /v1/models")
          jq -r '.data[0:5][]?.id' "${tmp_body}" | sed 's/^/  - /'
          ;;
        "POST /v1/responses")
          jq '{id, model, output_text}' "${tmp_body}"
          ;;
        "POST /v1/chat/completions")
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

for c in "${choices[@]}"; do
  case "${c}" in
    1)
      run_request "GET /v1/models" "GET" "https://api.openai.com/v1/models"
      ;;
    2)
      run_request "POST /v1/responses" "POST" "https://api.openai.com/v1/responses" \
        "{\"model\":\"${MODEL}\",\"input\":\"Reply with: pong\"}"
      ;;
    3)
      run_request "POST /v1/chat/completions" "POST" "https://api.openai.com/v1/chat/completions" \
        "{\"model\":\"${MODEL}\",\"messages\":[{\"role\":\"user\",\"content\":\"Reply with: pong\"}]}"
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
