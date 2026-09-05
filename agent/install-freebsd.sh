#!/bin/sh
set -eu

SERVICE_NAME="nodeflare-agent"
RC_NAME="nodeflare_agent"
INSTALL_DIR="/usr/local/libexec/nodeflare"
AGENT_FILE="$INSTALL_DIR/agent"
STATE_ROOT="/var/db/nodeflare"
STATE_DIR="$STATE_ROOT/agent"
SERVICE_FILE="/usr/local/etc/rc.d/$SERVICE_NAME"
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
    service "$SERVICE_NAME" stop 2>/dev/null || true
    if [ "$had_agent" = true ]; then
      cp -p "$backup_agent" "$AGENT_FILE" 2>/dev/null || true
    else
      rm -f "$AGENT_FILE"
    fi
    if [ "$had_service" = true ]; then
      cp -p "$backup_service" "$SERVICE_FILE" 2>/dev/null || true
      service "$SERVICE_NAME" start 2>/dev/null || true
    else
      rm -f "$SERVICE_FILE"
    fi
  fi
  [ -z "$temporary" ] || rm -f "$temporary"
  [ -z "$backup_agent" ] || rm -f "$backup_agent"
  [ -z "$backup_service" ] || rm -f "$backup_service"
}

usage() {
  cat <<'EOF'
NodeFlare Agent FreeBSD 安装脚本

用法：
  install-freebsd.sh -e <NodeFlare URL> -t <Agent Token> [-i <历史保存间隔>] [-m <下载加速前缀>]
  install-freebsd.sh --status
  install-freebsd.sh --uninstall

支持 FreeBSD amd64 和 arm64。Agent Token 请勿泄露。
-m 为可选的 GitHub 下载加速前缀（形如 https://ghproxy.net），
仅作用于 Release 下载，摘要校验不受影响。
EOF
}

safe_value() {
  case "$1" in *[!A-Za-z0-9_./:@-]*|'') return 1 ;; esac
}

download_stdout() {
  url="$1"
  if command -v curl >/dev/null 2>&1; then
    curl --fail --location --silent --show-error --max-time 30 \
      -H 'Accept: application/vnd.github+json' \
      -H 'User-Agent: nodeflare-installer' \
      "$url"
  else
    fetch -q -T 30 -o - "$url"
  fi
}

download_file() {
  url="$1"
  destination="$2"
  if command -v curl >/dev/null 2>&1; then
    curl --fail --location --silent --show-error --max-time 120 \
      "$url" -o "$destination"
  else
    fetch -q -T 120 -o "$destination" "$url"
  fi
}

if [ "${1:-}" = "--uninstall" ]; then
  [ "$#" -eq 1 ] || fail "--uninstall 不接受其它参数"
  [ "$(id -u)" -eq 0 ] || fail "请使用 root 权限执行卸载"
  log "正在停止并移除 NodeFlare Agent"
  service "$SERVICE_NAME" stop 2>/dev/null || true
  sysrc -x "${RC_NAME}_enable" >/dev/null 2>&1 || true
  rm -f "$SERVICE_FILE" "$AGENT_FILE"
  rm -rf "$STATE_DIR"
  rmdir "$STATE_ROOT" 2>/dev/null || true
  rmdir "$INSTALL_DIR" 2>/dev/null || true
  echo "NodeFlare Agent 已卸载"
  exit 0
fi

if [ "${1:-}" = "--status" ]; then
  [ "$#" -eq 1 ] || fail "--status 不接受其它参数"
  service "$SERVICE_NAME" status
  exit $?
fi

[ "${1:-}" != "-h" ] && [ "${1:-}" != "--help" ] && [ "$#" -gt 0 ] || {
  usage
  exit 0
}
[ "$(id -u)" -eq 0 ] || fail "请使用 root 权限运行安装"
[ "$(uname -s)" = "FreeBSD" ] || fail "此脚本仅支持 FreeBSD"
case "$(uname -m)" in
  amd64|x86_64) arch=x64 ;;
  arm64|aarch64) arch=aarch64 ;;
  *) fail "仅支持 FreeBSD amd64 和 arm64" ;;
esac
command -v fetch >/dev/null 2>&1 || command -v curl >/dev/null 2>&1 || fail "缺少 fetch 或 curl"
command -v sha256 >/dev/null 2>&1 || fail "缺少 sha256，无法校验下载文件"
command -v service >/dev/null 2>&1 || fail "缺少 service"
command -v sysrc >/dev/null 2>&1 || fail "缺少 sysrc"

log "正在检查运行环境"
token=""
endpoint=""
interval=60
interval_set=false
mirror=""
while [ "$#" -gt 0 ]; do
  option="$1"
  case "$option" in
    -t|-e|-i|-m)
      [ "$#" -ge 2 ] || fail "参数 $option 缺少值"
      value="$2"
      shift 2
      ;;
    *) fail "未知参数：$option" ;;
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

mkdir -p "$INSTALL_DIR" "$STATE_ROOT" "$STATE_DIR"
chmod 755 "$INSTALL_DIR"
chmod 750 "$STATE_ROOT"
chmod 750 "$STATE_DIR"
temporary="$INSTALL_DIR/.agent.$$.download"
trap cleanup_agent_install EXIT HUP INT TERM
artifact="agent-freebsd-$arch"
release_api="https://api.github.com/repos/imengying/NodeFlare/releases/latest"
log "正在获取 GitHub 最新正式版本（$artifact）"
release_json=$(download_stdout "$release_api")
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
download_file "$download_url" "$temporary"
actual=$(sha256 -q "$temporary")
[ "$actual" = "$expected" ] || fail "Agent SHA-256 校验失败，已停止安装"
chmod 755 "$temporary"
log "下载校验通过，正在验证可执行文件"
installed_version=$("$temporary" --version) || fail "下载的 Agent 无法在当前 FreeBSD 运行"
installed_version=${installed_version##* }
[ "$installed_version" = "${release_tag#v}" ] || fail "Release $release_tag 与 Agent 版本 $installed_version 不一致"

backup_agent="$STATE_DIR/.agent.$$.previous"
backup_service="$STATE_DIR/.service.$$.previous"
if [ -f "$AGENT_FILE" ]; then
  cp -p "$AGENT_FILE" "$backup_agent"
  had_agent=true
fi
if [ -f "$SERVICE_FILE" ]; then
  cp -p "$SERVICE_FILE" "$backup_service"
  had_service=true
fi
rollback_agent=true

log "正在配置并启动 FreeBSD rc.d 服务"
service "$SERVICE_NAME" stop 2>/dev/null || true
mv "$temporary" "$AGENT_FILE"
cat > "$SERVICE_FILE" <<EOF
#!/bin/sh
# PROVIDE: nodeflare_agent
# REQUIRE: NETWORKING
# KEYWORD: shutdown

. /etc/rc.subr

export NODEFLARE_STATE_DIR="$STATE_DIR"
export NODEFLARE_AGENT_TOKEN="$token"

name="$RC_NAME"
rcvar="${RC_NAME}_enable"
pidfile="/var/run/\${name}.pid"
command="/usr/sbin/daemon"
command_args="-P \${pidfile} -r -R 10 -S -T \${name} $AGENT_FILE -e $endpoint -i $interval"

load_rc_config "\${name}"
: \${nodeflare_agent_enable:="NO"}
run_rc_command "\$1"
EOF
chmod 700 "$SERVICE_FILE"
sysrc "${RC_NAME}_enable=YES" >/dev/null
service "$SERVICE_NAME" start
service "$SERVICE_NAME" status >/dev/null || {
  service "$SERVICE_NAME" status >&2 || true
  fail "NodeFlare 服务启动失败，请查看 /var/log/messages"
}

rollback_agent=false
cleanup_agent_install
trap - EXIT HUP INT TERM

printf '\nNodeFlare Agent 安装完成\n'
printf '  版本：%s\n' "$installed_version"
printf '  服务：%s（FreeBSD rc.d）\n' "$SERVICE_NAME"
printf '  查看状态：service %s status\n' "$SERVICE_NAME"
