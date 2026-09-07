#!/bin/sh
set -eu

SERVICE_NAME="nodeflare-agent"
INSTALL_DIR="/opt/nodeflare"
AGENT_FILE="$INSTALL_DIR/agent"
STATE_ROOT="/etc/nodeflare"
STATE_DIR="$STATE_ROOT/agent"
SERVICE_FILE="/etc/systemd/system/$SERVICE_NAME.service"
OPENRC_FILE="/etc/init.d/$SERVICE_NAME"
temporary=""
backup_agent=""
backup_service=""
rollback_agent=false
had_agent=false
had_service=false
update=false

usage() {
  cat <<'EOF'
NodeFlare Agent 安装脚本

用法：
  agent.sh -e <NodeFlare URL> -t <Agent Token> [-i <历史保存间隔>] [-m <下载加速前缀>]
  agent.sh --update [-m <下载加速前缀>]
  agent.sh --status
  agent.sh --uninstall

参数：
  -e  NodeFlare 服务地址（必填）
  -t  后台生成的 Agent Token（必填，请勿泄露）
  -i  初始历史保存间隔，15-3600 秒（默认 60）
  -m  GitHub 下载加速前缀（可选，如 https://ghproxy.net）
EOF
}

log() {
  printf '%s\n' "$1"
}

fail() {
  printf '错误：%s\n' "$1" >&2
  exit 1
}

print_install_result() {
  if [ "$update" = true ] || [ "$had_agent" = true ]; then
    printf '\n更新完成（v%s）\n' "$installed_version"
    return
  fi
  printf '\n安装完成（v%s）\n' "$installed_version"
  printf '  服务：%s（%s）\n' "$SERVICE_NAME" "$init_system"
  if [ "$init_system" = systemd ]; then
    printf '  查看状态：systemctl status %s\n' "$SERVICE_NAME"
    printf '  查看日志：journalctl -u %s -f\n' "$SERVICE_NAME"
  else
    printf '  查看状态：rc-service %s status\n' "$SERVICE_NAME"
  fi
}

cleanup_agent_install() {
  if [ "$rollback_agent" = true ]; then
    rollback_agent=false
    log "安装未完成，正在恢复上一版本"
    case "$init_system" in
      systemd) systemctl stop "$SERVICE_NAME" 2>/dev/null || true ;;
      openrc) rc-service "$SERVICE_NAME" stop 2>/dev/null || true ;;
    esac
    if [ "$had_agent" = true ]; then
      cp -p "$backup_agent" "$AGENT_FILE" 2>/dev/null || true
    else
      rm -f "$AGENT_FILE"
    fi
    service_path=$SERVICE_FILE
    [ "$init_system" != openrc ] || service_path=$OPENRC_FILE
    if [ "$had_service" = true ]; then
      cp -p "$backup_service" "$service_path" 2>/dev/null || true
    else
      rm -f "$service_path"
    fi
    case "$init_system" in
      systemd)
        systemctl daemon-reload 2>/dev/null || true
        [ "$had_agent" != true ] || systemctl restart "$SERVICE_NAME" 2>/dev/null || true
        ;;
      openrc) [ "$had_agent" != true ] || rc-service "$SERVICE_NAME" restart 2>/dev/null || true ;;
    esac
  fi
  [ -z "$temporary" ] || rm -f "$temporary"
  [ -z "$backup_agent" ] || rm -f "$backup_agent"
  [ -z "$backup_service" ] || rm -f "$backup_service"
}

detect_glibc_version() {
  [ -x "$glibc_loader" ] || return
  LC_ALL=C "$glibc_loader" --version 2>/dev/null | awk '
    tolower($0) ~ /glibc|gnu libc|gnu c library/ {
      sub(/\.$/, "")
      if (match($0, /[0-9]+\.[0-9]+(\.[0-9]+)?$/)) {
        print substr($0, RSTART, RLENGTH)
        exit
      }
    }
  '
}

glibc_is_supported() {
  glibc_major=${1%%.*}
  glibc_minor=${1#*.}
  glibc_minor=${glibc_minor%%.*}
  case "$glibc_major:$glibc_minor" in
    *[!0-9:]*|:*) return 1 ;;
  esac
  [ "$glibc_major" -gt 2 ] \
    || { [ "$glibc_major" -eq 2 ] && [ "$glibc_minor" -ge 28 ]; }
}

safe_value() {
  case "$1" in *[!A-Za-z0-9_./:@-]*|'') return 1 ;; esac
}

detect_init_system() {
  if [ -f /etc/alpine-release ] && command -v rc-service >/dev/null 2>&1; then
    printf 'openrc'
  elif [ -d /run/systemd/system ] && command -v systemctl >/dev/null 2>&1; then
    printf 'systemd'
  elif command -v rc-service >/dev/null 2>&1 && [ -d /etc/init.d ]; then
    printf 'openrc'
  else
    printf 'unknown'
  fi
}

ensure_curl() {
  command -v curl >/dev/null 2>&1 && return
  log "未找到 curl，正在通过系统包管理器安装"
  if command -v apt-get >/dev/null 2>&1; then
    apt-get update
    DEBIAN_FRONTEND=noninteractive apt-get install -y curl
  elif command -v dnf >/dev/null 2>&1; then
    dnf install -y curl
  elif command -v yum >/dev/null 2>&1; then
    yum install -y curl
  elif command -v apk >/dev/null 2>&1; then
    apk add --no-cache curl
  else
    fail "缺少 curl，且未找到受支持的包管理器"
  fi
  command -v curl >/dev/null 2>&1 || fail "curl 安装失败"
}

explain_exec_failure() {
  target="$1"
  if [ ! -f "$target" ]; then
    echo "下载文件在校验前丢失：$target" >&2
  elif [ ! -x "$target" ]; then
    echo "下载文件不可执行：$target" >&2
  elif [ "$(dd if="$target" bs=4 count=1 2>/dev/null)" != "$(printf '\177ELF')" ]; then
    echo "下载文件不是有效的 Linux ELF 可执行文件" >&2
  else
    echo "下载的 Agent 无法在当前系统运行，请确认系统架构与 Release 文件匹配" >&2
  fi
}

install_agent() {
  [ "$(id -u)" -eq 0 ] || fail "请使用 root 权限运行安装命令"
  log "正在检查运行环境"
  ensure_curl
  command -v sha256sum >/dev/null || fail "缺少 sha256sum，无法校验下载文件"
  token=""
  endpoint=""
  interval=60
  interval_set=false
  mirror=""
  while [ "$#" -gt 0 ]; do
    option="$1"
    case "$option" in
      -t|-e|-i|-m)
        [ "$#" -ge 2 ] || { echo "参数 $option 缺少值" >&2; usage; exit 1; }
        value="$2"
        shift 2
        ;;
      *) echo "未知参数：$option" >&2; usage; exit 1 ;;
    esac
    case "$option" in
      -t) [ -z "$token" ] || fail "参数 $option 重复"; token="$value" ;;
      -e) [ -z "$endpoint" ] || fail "参数 $option 重复"; endpoint="$value" ;;
      -i) [ "$interval_set" = false ] || fail "参数 $option 重复"; interval="$value"; interval_set=true ;;
      -m) [ -z "$mirror" ] || fail "参数 $option 重复"; mirror="$value" ;;
    esac
  done
  [ -n "$token" ] && [ -n "$endpoint" ] || { usage; exit 1; }
  endpoint=${endpoint%/}
  [ ${#token} -le 512 ] && [ ${#endpoint} -le 2048 ] || fail "安装参数长度超出限制"
  if ! safe_value "$token" || ! safe_value "$endpoint"; then
    fail "服务地址或 Agent Token 格式无效"
  fi
  case "$endpoint" in
    https://?*|http://localhost|http://localhost/*|http://localhost:*|http://127.0.0.1|http://127.0.0.1/*|http://127.0.0.1:*) ;;
    *) fail "服务地址必须使用 HTTPS；仅本机调试可使用 HTTP" ;;
  esac
  case "$endpoint" in *@*) fail "服务地址不能包含用户信息" ;; esac
  case "$interval" in ''|*[!0-9]*) fail "历史保存间隔必须是整数" ;; esac
  [ "$interval" -ge 15 ] && [ "$interval" -le 3600 ] || fail "历史保存间隔必须在 15-3600 秒之间"
  mirror=${mirror%/}
  if [ -n "$mirror" ]; then
    [ ${#mirror} -le 2048 ] || fail "下载加速前缀长度超出限制"
    safe_value "$mirror" || fail "下载加速前缀格式无效"
    case "$mirror" in
      https://?*) ;;
      http://localhost|http://localhost/*|http://localhost:*|http://127.0.0.1|http://127.0.0.1/*|http://127.0.0.1:*) ;;
      *) fail "下载加速前缀必须使用 HTTPS；仅本机调试可使用 HTTP" ;;
    esac
    case "$mirror" in *@*) fail "下载加速前缀不能包含用户信息" ;; esac
  fi
  case "$(uname -m)" in
    x86_64|amd64) arch="x64"; glibc_loader=/lib64/ld-linux-x86-64.so.2 ;;
    aarch64|arm64) arch="aarch64"; glibc_loader=/lib/ld-linux-aarch64.so.1 ;;
    *) fail "暂不支持当前 CPU 架构：$(uname -m)" ;;
  esac
  libc=musl
  glibc_version=$(detect_glibc_version || true)
  if [ -n "$glibc_version" ] && glibc_is_supported "$glibc_version"; then
    libc=glibc
  fi
  init_system=$(detect_init_system)
  [ "$init_system" != "unknown" ] || fail "未检测到正在运行的 systemd 或 OpenRC"

  mkdir -p "$INSTALL_DIR" "$STATE_ROOT" "$STATE_DIR"
  chmod 755 "$INSTALL_DIR"
  chmod 700 "$STATE_ROOT"
  chmod 750 "$STATE_DIR"
  temporary="$INSTALL_DIR/.agent.$$.download"
  trap cleanup_agent_install EXIT HUP INT TERM
  artifact="agent-linux-$arch-$libc"
  release_api="https://api.github.com/repos/imengying/NodeFlare/releases/latest"
  log "正在获取 GitHub 最新正式版本（$artifact）"
  release_json=$(curl --fail --location --silent --show-error --max-time 30 \
    -H 'Accept: application/vnd.github+json' \
    -H 'User-Agent: nodeflare-installer' \
    "$release_api")
  release_tag=$(printf '%s\n' "$release_json" | tr ',' '\n' | sed -n 's/.*"tag_name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' | head -n 1)
  printf '%s\n' "$release_tag" | grep -Eq '^v[0-9]+\.[0-9]+\.[0-9]+$' || {
    fail "GitHub 最新 Release 标签无效：${release_tag:-未找到}"
  }
  expected=$(printf '%s\n' "$release_json" | tr '{' '\n' | awk -v name="$artifact" '
    {
      compact = $0
      gsub(/[[:space:]]/, "", compact)
      if (index(compact, "\"name\":\"" name "\"") > 0) selected = 1
      else if (index(compact, "\"name\":") > 0) selected = 0
    }
    selected {
      marker = "\"digest\":\"sha256:"
      position = index(compact, marker)
      if (position == 0) next
      digest = substr(compact, position + length(marker), 64)
      if (length(digest) == 64 && digest !~ /[^0-9a-fA-F]/) {
        print tolower(digest)
        exit
      }
    }
  ')
  [ -n "$expected" ] || fail "Release 缺少 $artifact 的 SHA-256 摘要"
  release_base="https://github.com/imengying/NodeFlare/releases/download/$release_tag"
  download_url="$release_base/$artifact"
  if [ -n "$mirror" ]; then
    download_url="$mirror/$release_base/$artifact"
    log "正在通过下载加速前缀拉取 Agent $release_tag"
  else
    log "正在下载 Agent $release_tag"
  fi
  curl --fail --location --silent --show-error --max-time 120 \
    "$download_url" \
    -o "$temporary"
  actual=$(sha256sum "$temporary" | awk '{ print $1 }')
  [ -n "$expected" ] && [ "$actual" = "$expected" ] || fail "Agent SHA-256 校验失败，已停止安装"
  log "下载校验通过，正在验证可执行文件"
  chmod 755 "$temporary"
  if ! installed_version=$("$temporary" --version); then
    explain_exec_failure "$temporary"
    exit 1
  fi
  installed_version=${installed_version##* }
  printf '%s\n' "$installed_version" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+$' || {
    fail "Agent 返回了无效版本号：$installed_version"
  }
  [ "$installed_version" = "${release_tag#v}" ] || {
    fail "Release $release_tag 与 Agent 版本 $installed_version 不一致"
  }
  backup_agent="$STATE_DIR/.agent.$$.previous"
  service_path=$SERVICE_FILE
  [ "$init_system" != openrc ] || service_path=$OPENRC_FILE
  backup_service="$STATE_DIR/.service.$$.previous"
  if [ -f "$AGENT_FILE" ]; then
    cp -p "$AGENT_FILE" "$backup_agent"
    had_agent=true
  fi
  if [ -f "$service_path" ]; then
    cp -p "$service_path" "$backup_service"
    had_service=true
  fi
  rollback_agent=true
  case "$init_system" in
    systemd) systemctl stop "$SERVICE_NAME" 2>/dev/null || true ;;
    openrc) rc-service "$SERVICE_NAME" stop 2>/dev/null || true ;;
  esac
  mv "$temporary" "$AGENT_FILE"
  log "正在启动 $init_system 服务"
  if [ "$init_system" = "systemd" ]; then
    printf '%s\n' \
    '[Unit]' \
    'Description=NodeFlare Agent' \
    'After=network-online.target' \
    'Wants=network-online.target' \
    '' \
    '[Service]' \
    'Type=simple' \
    "ExecStart=$AGENT_FILE -e $endpoint -i $interval" \
    'Restart=always' \
    'RestartSec=10' \
    'NoNewPrivileges=true' \
    "Environment=NODEFLARE_AGENT_TOKEN=$token" \
    "Environment=NODEFLARE_STATE_DIR=$STATE_DIR" \
    '' \
    '[Install]' \
    'WantedBy=multi-user.target' > "$SERVICE_FILE"
    chmod 600 "$SERVICE_FILE"
    systemctl daemon-reload
    systemctl enable --now --quiet "$SERVICE_NAME"
    systemctl is-active --quiet "$SERVICE_NAME" || {
      systemctl status "$SERVICE_NAME" --no-pager >&2 || true
      fail "服务启动失败，请查看上方状态信息"
    }
  elif [ "$init_system" = "openrc" ]; then
    printf '%s\n' \
      '#!/sbin/openrc-run' \
      "name=\"$SERVICE_NAME\"" \
      "command=\"$AGENT_FILE\"" \
      "command_args=\"-e $endpoint -i $interval\"" \
      "command_user=\"root\"" \
      "export NODEFLARE_AGENT_TOKEN=\"$token\"" \
      "export NODEFLARE_STATE_DIR=\"$STATE_DIR\"" \
      "supervisor=\"supervise-daemon\"" \
      "respawn_delay=10" \
      'depend() { need net; }' > "$OPENRC_FILE"
    chmod 700 "$OPENRC_FILE"
    rc-update add "$SERVICE_NAME" default
    rc-service "$SERVICE_NAME" restart
    rc-service "$SERVICE_NAME" status >/dev/null || {
      rc-service "$SERVICE_NAME" status >&2 || true
      fail "服务启动失败，请查看上方状态信息"
    }
  fi
  rollback_agent=false
  cleanup_agent_install
  trap - EXIT HUP INT TERM
  print_install_result
}

load_installed_agent_config() {
  case "$init_system" in
    systemd)
      [ -f "$SERVICE_FILE" ] || fail "未找到 Agent 服务配置，请先完成安装"
      service_exec=$(sed -n 's/^ExecStart=//p' "$SERVICE_FILE" | head -n 1)
      endpoint=$(printf '%s\n' "$service_exec" | sed -n 's/.* -e \([^[:space:]]*\) -i \([0-9][0-9]*\)$/\1/p')
      interval=$(printf '%s\n' "$service_exec" | sed -n 's/.* -e \([^[:space:]]*\) -i \([0-9][0-9]*\)$/\2/p')
      token=$(sed -n 's/^Environment=NODEFLARE_AGENT_TOKEN=//p' "$SERVICE_FILE" | head -n 1)
      ;;
    openrc)
      [ -f "$OPENRC_FILE" ] || fail "未找到 Agent 服务配置，请先完成安装"
      command_args=$(sed -n 's/^command_args="\([^"]*\)"$/\1/p' "$OPENRC_FILE" | head -n 1)
      endpoint=$(printf '%s\n' "$command_args" | sed -n 's/^-e \([^[:space:]]*\) -i \([0-9][0-9]*\)$/\1/p')
      interval=$(printf '%s\n' "$command_args" | sed -n 's/^-e \([^[:space:]]*\) -i \([0-9][0-9]*\)$/\2/p')
      token=$(sed -n 's/^export NODEFLARE_AGENT_TOKEN="\([^"]*\)"$/\1/p' "$OPENRC_FILE" | head -n 1)
      ;;
    *)
      fail "未检测到正在运行的 systemd 或 OpenRC，无法自动读取 Agent 配置"
      ;;
  esac
  [ -n "${endpoint:-}" ] && [ -n "${token:-}" ] && [ -n "${interval:-}" ] || {
    fail "无法读取现有 Agent 配置，请改用安装参数 -e 和 -t"
  }
}

update_agent() {
  [ "$(id -u)" -eq 0 ] || fail "请使用 root 权限执行更新"
  update=true
  mirror=""
  while [ "$#" -gt 0 ]; do
    option="$1"
    case "$option" in
      -m)
        [ "$#" -ge 2 ] || fail "参数 -m 缺少值"
        [ -z "$mirror" ] || fail "参数 -m 重复"
        mirror="$2"
        shift 2
        ;;
      *)
        echo "未知参数：$option" >&2
        usage
        exit 1
        ;;
    esac
  done
  init_system=$(detect_init_system)
  load_installed_agent_config
  if [ -n "$mirror" ]; then
    install_agent -e "$endpoint" -t "$token" -i "$interval" -m "$mirror"
  else
    install_agent -e "$endpoint" -t "$token" -i "$interval"
  fi
}

status_agent() {
  init_system=$(detect_init_system)
  if [ "$init_system" = "systemd" ]; then
    if systemctl status "$SERVICE_NAME" --no-pager; then return 0; else return $?; fi
  fi
  if [ "$init_system" = "openrc" ]; then
    rc-service "$SERVICE_NAME" status
    return $?
  fi
  echo "未检测到 Agent 服务" >&2
  return 1
}

uninstall_agent() {
  [ "$(id -u)" -eq 0 ] || fail "请使用 root 权限执行卸载"
  log "正在停止并移除 Agent 服务"
  init_system=$(detect_init_system)
  case "$init_system" in
    systemd) systemctl disable --now "$SERVICE_NAME" 2>/dev/null || true ;;
    openrc)
      rc-service "$SERVICE_NAME" stop 2>/dev/null || true
      rc-update del "$SERVICE_NAME" default 2>/dev/null || true
      ;;
  esac
  rm -f "$SERVICE_FILE"
  rm -f "$OPENRC_FILE"
  [ "$init_system" != "systemd" ] || systemctl daemon-reload 2>/dev/null || true
  rm -f "$AGENT_FILE"
  rm -rf "$STATE_DIR"
  rmdir "$STATE_ROOT" 2>/dev/null || true
  rmdir "$INSTALL_DIR" 2>/dev/null || true
  echo "Agent 已卸载"
}

case "${1:-}" in
  --update) shift; update_agent "$@" ;;
  --uninstall) [ "$#" -eq 1 ] || { usage; exit 1; }; uninstall_agent ;;
  --status) [ "$#" -eq 1 ] || { usage; exit 1; }; status_agent ;;
  -h|--help|'') usage; exit 0 ;;
  *) install_agent "$@" ;;
esac
