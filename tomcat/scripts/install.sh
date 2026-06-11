#!/usr/bin/env bash
# tomcat 一键安装脚本（macOS/Linux）。
# 默认安装最新 release 到 ~/.local/bin，并可按需将 PATH 写入 shell profile。
# 支持 -y/--yes：非交互模式，自动追加 PATH。支持 -v VER：指定版本（接受 v0.1.3 或 0.1.3）。
set -euo pipefail

REPO="teisun/tomcat-agent"
INSTALL_DIR="$HOME/.local/bin"
NON_INTERACTIVE=0
TAG=""

usage() {
  cat <<'EOF'
Usage: install.sh [-y|--yes] [-v VERSION]

Options:
  -y, --yes        非交互模式；若 ~/.local/bin 不在 PATH 中，自动写入 shell profile
  -v VERSION       安装指定版本，例如 v0.1.3 或 0.1.3
  -h, --help       显示帮助
EOF
}

require_command() {
  if ! command -v "$1" >/dev/null 2>&1; then
    echo "缺少依赖命令: $1" >&2
    exit 1
  fi
}

normalize_tag() {
  case "$1" in
    v*) printf '%s\n' "$1" ;;
    *) printf 'v%s\n' "$1" ;;
  esac
}

detect_target() {
  local os arch
  os="$(uname -s)"
  arch="$(uname -m)"

  case "${os}" in
    Darwin)
      case "${arch}" in
        arm64|aarch64) printf 'aarch64-apple-darwin\n' ;;
        x86_64|amd64) printf 'x86_64-apple-darwin\n' ;;
        *)
          echo "暂不支持的 macOS 架构: ${arch}" >&2
          exit 1
          ;;
      esac
      ;;
    Linux)
      case "${arch}" in
        x86_64|amd64) printf 'x86_64-unknown-linux-gnu\n' ;;
        *)
          echo "暂不支持的 Linux 架构: ${arch}" >&2
          exit 1
          ;;
      esac
      ;;
    *)
      echo "暂不支持的操作系统: ${os}" >&2
      echo "目前仅支持 macOS 与 Linux。" >&2
      exit 1
      ;;
  esac
}

resolve_latest_tag() {
  local api_url body tag
  api_url="https://api.github.com/repos/${REPO}/releases/latest"
  body="$(curl -fsSL "${api_url}" | tr -d '\n')"
  tag="$(printf '%s' "${body}" | sed -n 's/.*"tag_name":[[:space:]]*"\([^"]*\)".*/\1/p')"

  if [ -z "${tag}" ]; then
    echo "无法解析最新 release 版本，请稍后重试或使用 -v 指定版本。" >&2
    exit 1
  fi

  printf '%s\n' "${tag}"
}

sha256_file() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{print $1}'
  else
    shasum -a 256 "$1" | awk '{print $1}'
  fi
}

verify_checksum() {
  local checksum_file asset_path asset_name expected actual
  checksum_file="$1"
  asset_path="$2"
  asset_name="$(basename "${asset_path}")"

  expected="$(awk -v file="${asset_name}" '$2 == file { print $1 }' "${checksum_file}")"
  if [ -z "${expected}" ]; then
    echo "校验文件中缺少 ${asset_name} 的 SHA256 记录。" >&2
    exit 1
  fi

  actual="$(sha256_file "${asset_path}")"
  if [ "${expected}" != "${actual}" ]; then
    echo "SHA256 校验失败: ${asset_name}" >&2
    echo "expected: ${expected}" >&2
    echo "actual:   ${actual}" >&2
    exit 1
  fi
}

detect_profile() {
  case "${SHELL:-}" in
    *zsh) printf '%s\n' "$HOME/.zshrc" ;;
    *bash) printf '%s\n' "$HOME/.bashrc" ;;
    *) printf '%s\n' "$HOME/.profile" ;;
  esac
}

ensure_path_config() {
  local profile export_line do_append answer
  export_line='export PATH="$HOME/.local/bin:$PATH"'

  case ":$PATH:" in
    *":${INSTALL_DIR}:"*)
      echo "检测到 ${INSTALL_DIR} 已在 PATH 中。"
      return 0
      ;;
  esac

  profile="$(detect_profile)"
  do_append=0

  if [ "${NON_INTERACTIVE}" -eq 1 ]; then
    do_append=1
  else
    echo ""
    printf "是否将 %s 追加到 %s，使新开终端可直接执行 tomcat？[y/N] " "${export_line}" "${profile}"
    read -r answer
    case "${answer:-n}" in
      [yY]|[yY][eE][sS]) do_append=1 ;;
    esac
  fi

  if [ "${do_append}" -eq 1 ]; then
    if [ -f "${profile}" ] && grep -Fqs "${export_line}" "${profile}" 2>/dev/null; then
      echo "PATH 配置已存在于 ${profile}，跳过追加。"
    else
      {
        echo ""
        echo "# tomcat (install.sh)"
        echo "${export_line}"
      } >> "${profile}"
      echo "已写入 ${profile}，新开终端将自动生效。"
    fi
    echo "当前终端请执行: source \"${profile}\"  或重新打开终端。"
  else
    echo "当前终端可直接执行: \"${INSTALL_DIR}/tomcat\" init"
    echo "若想直接使用 tomcat 命令，请将以下内容加入 ${profile}:"
    echo "  ${export_line}"
  fi
}

while [ $# -gt 0 ]; do
  case "$1" in
    -y|--yes)
      NON_INTERACTIVE=1
      shift
      ;;
    -v)
      if [ $# -lt 2 ]; then
        echo "-v 需要传入版本号，例如 -v v0.1.3" >&2
        exit 1
      fi
      TAG="$(normalize_tag "$2")"
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "未知参数: $1" >&2
      usage >&2
      exit 1
      ;;
  esac
done

require_command curl
require_command tar
require_command awk
require_command sed
if ! command -v sha256sum >/dev/null 2>&1 && ! command -v shasum >/dev/null 2>&1; then
  echo "缺少 SHA256 工具：请安装 sha256sum 或 shasum。" >&2
  exit 1
fi

TARGET="$(detect_target)"
if [ -z "${TAG}" ]; then
  TAG="$(resolve_latest_tag)"
fi

TMP_DIR="$(mktemp -d)"
trap 'rm -rf "${TMP_DIR}"' EXIT

ASSET_NAME="tomcat-${TAG}-${TARGET}.tar.gz"
ASSET_URL="https://github.com/${REPO}/releases/download/${TAG}/${ASSET_NAME}"
CHECKSUM_URL="https://github.com/${REPO}/releases/download/${TAG}/SHA256SUMS"
ASSET_PATH="${TMP_DIR}/${ASSET_NAME}"
CHECKSUM_PATH="${TMP_DIR}/SHA256SUMS"

echo "准备安装 tomcat ${TAG} (${TARGET}) 到 ${INSTALL_DIR}"
mkdir -p "${INSTALL_DIR}"

echo "下载校验文件..."
curl -fsSL "${CHECKSUM_URL}" -o "${CHECKSUM_PATH}"

echo "下载安装包..."
curl -fL "${ASSET_URL}" -o "${ASSET_PATH}"

echo "校验 SHA256..."
verify_checksum "${CHECKSUM_PATH}" "${ASSET_PATH}"

echo "解压安装包..."
tar -xzf "${ASSET_PATH}" -C "${TMP_DIR}"

if [ ! -f "${TMP_DIR}/tomcat" ]; then
  echo "安装包中未找到 tomcat 二进制。" >&2
  exit 1
fi

chmod +x "${TMP_DIR}/tomcat"
mv "${TMP_DIR}/tomcat" "${INSTALL_DIR}/tomcat"

echo "安装完成: ${INSTALL_DIR}/tomcat"
ensure_path_config
echo ""
echo "下一步请运行: ${INSTALL_DIR}/tomcat init"
echo "如当前 shell 已包含 ${INSTALL_DIR}，也可直接运行: tomcat init"
