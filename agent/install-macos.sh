#!/bin/sh
set -eu

LABEL="nodeflare-agent"
INSTALL_DIR="/usr/local/libexec/nodeflare"
AGENT_FILE="$INSTALL_DIR/agent"
STATE_ROOT="/Library/Application Support/NodeFlare"
STATE_DIR="$STATE_ROOT/Agent"
PLIST_FILE="/Library/LaunchDaemons/$LABEL.plist"
temporary=""
backup_agent=""
backup_service=""
rollback_agent=false
had_agent=false
had_service=false

log() {
  printf '[NodeFlare] %s\n' "$1"
}

fail() {
  printf '[NodeFlare] 错误：%s\n' "$1" >&2
  exit 1
}

cleanup_agent_install() {
  if [ "$rollback_agent" = true ]; then
    rollback_agent=false
    log "安装未完成，正在恢复上一版本"
    launchctl bootout system "$PLIST_FILE" 2>/dev/null || true
    if [ "$had_agent" = true ]; then
      cp -p "$backup_agent" "$AGENT_FILE" 2>/dev/null || true
    else
      rm -f "$AGENT_FILE"
    fi
    if [ "$had_service" = true ]; then
      cp -p "$backup_service" "$PLIST_FILE" 2>/dev/null || true
      launchctl bootstrap system "$PLIST_FILE" 2>/dev/null || true
    else
      rm -f "$PLIST_FILE"
    fi
  fi
  [ -z "$temporary" ] || rm -f "$temporary"
  [ -z "$backup_agent" ] || rm -f "$backup_agent"
  [ -z "$backup_service" ] || rm -f "$backup_service"
}

usage() {
  cat <<'EOF'
NodeFlare Agent macOS 安装脚本

用法：
  install-macos.sh -e <NodeFlare URL> -t <Agent Token> [-i <历史保存间隔>] [-m <下载加速前缀>]
  install-macos.sh --update [-m <下载加速前缀>]
  install-macos.sh --status
  install-macos.sh --uninstall

仅支持 Apple Silicon（arm64）。Agent Token 请勿泄露。
-m 为可选的 GitHub 下载加速前缀（如 https://ghproxy.net）。
--update 沿用已安装 Agent 的服务地址、Token 和历史保存间隔。
EOF
}

safe_value() {
  case "$1" in *[!A-Za-z0-9_./:@-]*|'') return 1 ;; esac
}

plist_value() {
  /usr/libexec/PlistBuddy -c "Print :$1" "$PLIST_FILE"
}

load_installed_agent_config() {
  [ -f "$PLIST_FILE" ] || fail "未找到已安装 Agent 的服务配置，请先安装"
  [ "$(plist_value ProgramArguments:0)" = "$AGENT_FILE" ] &&
    [ "$(plist_value ProgramArguments:1)" = "-e" ] &&
    [ "$(plist_value ProgramArguments:3)" = "-i" ] || fail "无法识别已安装 Agent 的启动参数"
  endpoint=$(plist_value ProgramArguments:2) || fail "无法读取已安装 Agent 的服务地址"
  interval=$(plist_value ProgramArguments:4) || fail "无法读取已安装 Agent 的历史保存间隔"
  token=$(plist_value EnvironmentVariables:NODEFLARE_AGENT_TOKEN) || fail "无法读取已安装 Agent 的 Token"
  [ -n "$endpoint" ] && [ -n "$token" ] && [ -n "$interval" ] || fail "已安装 Agent 的服务配置不完整"
}

parse_agent_args() {
  token=""
  endpoint=""
  interval=60
  interval_set=false
  mirror=""
  mirror_set=false
  update=false
  if [ "${1:-}" = "--update" ]; then
    update=true
    shift
  fi
  while [ "$#" -gt 0 ]; do
    option="$1"
    if [ "$update" = true ] && [ "$option" != "-m" ]; then
      fail "--update 仅接受可选的 -m 下载加速前缀"
    fi
    case "$option" in
      -t|-e|-i|-m)
        [ "$#" -ge 2 ] && [ -n "$2" ] || fail "参数 $option 缺少值"
        value="$2"
        shift 2
        ;;
      *) fail "未知参数：$option" ;;
    esac
    case "$option" in
      -t) [ -z "$token" ] || fail "参数 $option 重复"; token="$value" ;;
      -e) [ -z "$endpoint" ] || fail "参数 $option 重复"; endpoint="$value" ;;
      -i) [ "$interval_set" = false ] || fail "参数 $option 重复"; interval="$value"; interval_set=true ;;
      -m) [ "$mirror_set" = false ] || fail "参数 $option 重复"; mirror="$value"; mirror_set=true ;;
    esac
  done
  if [ "$update" = true ]; then
    load_installed_agent_config
  fi
}

if [ "${1:-}" = "--uninstall" ]; then
  [ "$#" -eq 1 ] || fail "--uninstall 不接受其它参数"
  [ "$(id -u)" -eq 0 ] || fail "请使用 root 权限执行卸载"
  log "正在停止并移除 NodeFlare Agent"
  launchctl bootout system "$PLIST_FILE" 2>/dev/null || true
  rm -f "$PLIST_FILE" "$AGENT_FILE"
  rm -rf "$STATE_DIR"
  rmdir "$STATE_ROOT" 2>/dev/null || true
  rmdir "$INSTALL_DIR" 2>/dev/null || true
  echo "NodeFlare Agent 已卸载"
  exit 0
fi

if [ "${1:-}" = "--status" ]; then
  [ "$#" -eq 1 ] || fail "--status 不接受其它参数"
  launchctl print "system/$LABEL"
  exit $?
fi

[ "${1:-}" != "-h" ] && [ "${1:-}" != "--help" ] && [ "$#" -gt 0 ] || {
  usage
  exit 0
}
[ "$(id -u)" -eq 0 ] || fail "请使用 root 权限运行安装"
[ "$(uname -m)" = "arm64" ] || fail "仅支持 Apple Silicon（arm64）"
command -v curl >/dev/null || fail "缺少 curl"
command -v shasum >/dev/null || fail "缺少 shasum，无法校验下载文件"
log "正在检查运行环境"
parse_agent_args "$@"
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

mkdir -p "$INSTALL_DIR" "$STATE_ROOT" "$STATE_DIR"
chmod 755 "$INSTALL_DIR"
chmod 750 "$STATE_ROOT"
chmod 750 "$STATE_DIR"
temporary="$INSTALL_DIR/.agent.$$.download"
trap cleanup_agent_install EXIT HUP INT TERM
artifact="agent-macos-aarch64"
release_api="https://api.github.com/repos/imengying/NodeFlare/releases/latest"
log "正在获取 GitHub 最新正式版本（$artifact）"
release_json=$(curl --fail --location --silent --show-error --max-time 30 \
  -H 'Accept: application/vnd.github+json' \
  -H 'User-Agent: nodeflare-installer' \
  "$release_api")
release_tag=$(printf '%s\n' "$release_json" | tr ',' '\n' | sed -n 's/.*"tag_name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' | head -n 1)
printf '%s\n' "$release_tag" | grep -Eq '^v[0-9]+\.[0-9]+\.[0-9]+$' || fail "GitHub 最新 Release 标签无效：${release_tag:-未找到}"
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
  log "正在下载 NodeFlare Agent $release_tag"
fi
curl --fail --location --silent --show-error --max-time 120 \
  "$download_url" \
  -o "$temporary"
actual=$(shasum -a 256 "$temporary" | awk '{ print $1 }')
[ -n "$expected" ] && [ "$actual" = "$expected" ] || fail "Agent SHA-256 校验失败，已停止安装"
chmod 755 "$temporary"
log "下载校验通过，正在验证可执行文件"
installed_version=$("$temporary" --version) || fail "下载的 Agent 无法在当前 macOS 运行"
installed_version=${installed_version##* }
[ "$installed_version" = "${release_tag#v}" ] || fail "Release $release_tag 与 Agent 版本 $installed_version 不一致"
backup_agent="$STATE_DIR/.agent.$$.previous"
backup_service="$STATE_DIR/.service.$$.previous"
if [ -f "$AGENT_FILE" ]; then
  cp -p "$AGENT_FILE" "$backup_agent"
  had_agent=true
fi
if [ -f "$PLIST_FILE" ]; then
  cp -p "$PLIST_FILE" "$backup_service"
  had_service=true
fi
rollback_agent=true
log "正在配置并启动 macOS LaunchDaemon 服务"
launchctl bootout system "$PLIST_FILE" 2>/dev/null || true
mv "$temporary" "$AGENT_FILE"
cat > "$PLIST_FILE" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>Label</key><string>$LABEL</string>
<key>ProgramArguments</key><array><string>$AGENT_FILE</string><string>-e</string><string>$endpoint</string><string>-i</string><string>$interval</string></array>
<key>EnvironmentVariables</key><dict><key>NODEFLARE_AGENT_TOKEN</key><string>$token</string><key>NODEFLARE_STATE_DIR</key><string>$STATE_DIR</string></dict>
<key>KeepAlive</key><true/><key>RunAtLoad</key><true/>
<key>StandardOutPath</key><string>/var/log/nodeflare-agent.log</string>
<key>StandardErrorPath</key><string>/var/log/nodeflare-agent.log</string>
</dict></plist>
EOF
chmod 600 "$PLIST_FILE"
launchctl bootstrap system "$PLIST_FILE"
started=false
for _ in 1 2 3 4 5 6 7 8 9 10; do
  sleep 1
  if launchctl print "system/$LABEL" 2>/dev/null | grep -q 'state = running'; then
    started=true
    break
  fi
done
[ "$started" = true ] || fail "NodeFlare 服务启动失败，请查看 /var/log/nodeflare-agent.log"
rollback_agent=false
cleanup_agent_install
trap - EXIT HUP INT TERM
printf '\nNodeFlare Agent 安装完成\n'
printf '  版本：%s\n' "$installed_version"
printf '  服务：%s（LaunchDaemon）\n' "$LABEL"
printf '  查看状态：sudo launchctl print system/%s\n' "$LABEL"
printf '  查看日志：/var/log/nodeflare-agent.log\n'
