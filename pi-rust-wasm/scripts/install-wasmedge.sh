#!/usr/bin/env bash
# WasmEdge 安装脚本（Linux/macOS）。用户级安装后可选择将 PATH 写入 shell profile，使新开终端自动生效。
# Windows 请见 https://wasmedge.org/docs/start/install
set -e

# Windows 检测（Git Bash / WSL 下 uname 仍为 Linux，此处仅做常见判断；原生 Windows 建议用官方文档）
if [ -n "$OS" ] && [ "$OS" = "Windows_NT" ]; then
  echo "Windows 请见 https://wasmedge.org/docs/start/install，不在此脚本内实现。" >&2
  exit 1
fi

INSTALL_ARGS=()
while [ $# -gt 0 ]; do
  case "$1" in
    -p)
      INSTALL_ARGS+=("$1" "$2")
      shift 2
      ;;
    *)
      INSTALL_ARGS+=("$1")
      shift
      ;;
  esac
done

echo "正在调用 WasmEdge 官方安装脚本..."
if [ ${#INSTALL_ARGS[@]} -gt 0 ]; then
  curl -sSf https://raw.githubusercontent.com/WasmEdge/WasmEdge/master/utils/install.sh | bash -s -- "${INSTALL_ARGS[@]}"
else
  curl -sSf https://raw.githubusercontent.com/WasmEdge/WasmEdge/master/utils/install.sh | bash
fi

# 用户级安装（未传 -p /usr/local）时询问是否写入 profile，使新开终端自动检测到 wasmedge
SYSTEM_INSTALL=0
for i in "${!INSTALL_ARGS[@]}"; do
  [ "${INSTALL_ARGS[$i]}" = "-p" ] && SYSTEM_INSTALL=1 && break
done

if [ $SYSTEM_INSTALL -eq 0 ] && [ -d "$HOME/.wasmedge" ]; then
  WASMEDGE_ENV_LINE='source $HOME/.wasmedge/env'
  case "${SHELL:-}" in
    *zsh) PROFILE="$HOME/.zshrc" ;;
    *)    PROFILE="$HOME/.bashrc" ;;
  esac

  echo ""
  printf "是否将 %s 追加到 %s，使新开终端自动生效？[y/N] " "$WASMEDGE_ENV_LINE" "$PROFILE"
  read -r answer
  case "${answer:-n}" in
    [yY]|[yY][eE][sS])
      if [ -f "$PROFILE" ] && grep -q '\.wasmedge/env' "$PROFILE" 2>/dev/null; then
        echo "已存在 wasmedge env 配置，跳过追加。"
      else
        echo "" >> "$PROFILE"
        echo "# WasmEdge (install-wasmedge.sh)" >> "$PROFILE"
        echo "$WASMEDGE_ENV_LINE" >> "$PROFILE"
        echo "已写入 $PROFILE，新开终端将自动生效。"
      fi
      echo "当前终端请执行: source \$HOME/.wasmedge/env  或重新打开终端。"
      ;;
    *)
      echo "当前终端请执行: source \$HOME/.wasmedge/env"
      echo "新开终端每次需再次 source，或手动将上述一行加入 $PROFILE"
      ;;
  esac
elif [ $SYSTEM_INSTALL -eq 1 ]; then
  echo "系统级安装完成，无需 source。"
fi
