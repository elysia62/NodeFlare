#!/bin/sh
set -eu

repository=elysia62/NodeFlare
init_system=""
systemd_file=/etc/systemd/system/nodeflare.service
openrc_file=/etc/init.d/nodeflare
launchd_file=/Library/LaunchDaemons/nodeflare.plist
freebsd_rc_file=/usr/local/etc/rc.d/nodeflare
download_dir=""
package_dir=""
release_version=""
config_temp=""
tty_state=""
rollback_ready=false
backup_dir=""
previous_install=false
previous_share=false
previous_service=false
server_port=2206

case "$(uname -s)" in
  Linux)
    platform=linux
    config_dir=/etc/nodeflare
    install_dir=/opt/nodeflare
    share_dir=$install_dir/share
    default_database_url=sqlite:///etc/nodeflare/nodeflare.db
    ;;
  Darwin)
    platform=macos
    config_dir="/Library/Application Support/NodeFlare/Server"
    install_dir=/usr/local/libexec/nodeflare
    share_dir=$install_dir/share
    default_database_url=sqlite://nodeflare.db
    ;;
  FreeBSD)
    platform=freebsd
    config_dir=/var/db/nodeflare/server
    install_dir=/usr/local/libexec/nodeflare
    share_dir=/usr/local/share/nodeflare
    default_database_url=sqlite://nodeflare.db
    ;;
  *)
    printf '错误：当前系统不支持此安装脚本\n' >&2
    exit 1
    ;;
esac

config_file=$config_dir/config.toml
server_binary=$install_dir/nodeflare
theme_dir=$config_dir/themes
public_frontend_dir=$share_dir/frontend
admin_frontend_dir=$share_dir/admin

usage() {
  printf '%s\n' \
    'NodeFlare 面板安装脚本' \
    '' \
    '用法：' \
    '  sudo sh install.sh              # 交互菜单' \
    '  sudo sh install.sh --install    # 安装或更新' \
    '  sudo sh install.sh --status     # 查看状态' \
    '  sudo sh install.sh --restart    # 重启服务' \
    '  sudo sh install.sh --uninstall' \
    '  sudo sh install.sh --uninstall --purge' \
    '' \
    '首次安装会询问管理员用户名、密码、监听端口（默认 2206）和数据库连接。' \
    '安装和更新均使用 GitHub latest Release。' \
    '--uninstall 保留配置和数据；只有同时指定 --purge 才彻底删除。'
}

log() {
  printf '%s\n' "$1"
}

fail() {
  printf '错误：%s\n' "$1" >&2
  exit 1
}

print_install_result() {
  if [ "$new_config" = false ]; then
    printf '\n更新完成（v%s）\n' "$release_version"
    return
  fi
  printf '\n安装完成（v%s）\n' "$release_version"
  printf '%s\n' \
    "配置和数据：$config_dir" \
    "程序：$server_binary" \
    "服务系统：$init_system"
  printf '本机访问：http://127.0.0.1:%s/admin/login\n' "$server_port"
  printf '%s\n' "下一步：登录管理后台创建节点，并按弹窗命令安装 Agent"
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

detect_init_system() {
  case "$platform" in
    linux)
      if [ -d /run/systemd/system ] && command -v systemctl >/dev/null 2>&1; then
        printf systemd
      elif command -v rc-service >/dev/null 2>&1 && [ -d /etc/init.d ]; then
        printf openrc
      else
        printf unknown
      fi
      ;;
    macos) command -v launchctl >/dev/null 2>&1 && printf launchd || printf unknown ;;
    freebsd)
      command -v service >/dev/null 2>&1 && command -v sysrc >/dev/null 2>&1 \
        && printf freebsd || printf unknown
      ;;
  esac
}

restore_tty() {
  if [ -n "$tty_state" ]; then
    stty "$tty_state" < /dev/tty 2>/dev/null || true
    tty_state=""
  fi
}

cleanup() {
  restore_tty
  if [ "$rollback_ready" = true ]; then
    rollback_install
  fi
  [ -z "$config_temp" ] || rm -f "$config_temp"
  [ -z "$download_dir" ] || rm -rf "$download_dir"
}

trap cleanup EXIT
trap 'exit 129' HUP
trap 'exit 130' INT
trap 'exit 143' TERM

toml_escape() {
  printf '%s' "$1" | sed -e 's/\\/\\\\/g' -e 's/"/\\"/g'
}

prompt_line() {
  printf '%s' "$1" > /dev/tty
  IFS= read -r prompt_value < /dev/tty || fail "无法读取输入"
}

show_menu() {
  (: < /dev/tty) 2>/dev/null || fail "交互菜单需要终端；直接安装或更新请使用 --install"
  installed_version=""
  if [ -x "$server_binary" ]; then
    binary_version=$("$server_binary" --version 2>/dev/null || true)
    installed_version=${binary_version##* }
    printf '%s\n' "$installed_version" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+$' || installed_version=""
  fi
  if [ -n "$installed_version" ]; then
    install_action="更新（当前 v$installed_version）"
  else
    install_action="安装"
  fi
  printf '%s\n' \
    '' \
    'NodeFlare 面板管理' \
    "  1. $install_action" \
    '  2. 查看服务状态' \
    '  3. 重启服务' \
    '  4. 卸载（保留配置和数据）' \
    '  0. 退出' > /dev/tty
  while :; do
    prompt_line "请选择 [0]: "
    case "${prompt_value:-0}" in
      1) mode=install; return ;;
      2) mode=status; return ;;
      3) mode=restart; return ;;
      4)
        prompt_line "确认卸载 NodeFlare 面板？[y/N]: "
        case "$prompt_value" in
          y|Y) mode=uninstall ;;
          *) mode=exit ;;
        esac
        return
        ;;
      0) mode=exit; return ;;
      *) printf '%s\n' "请输入 0-4。" > /dev/tty ;;
    esac
  done
}

valid_port() {
  case "$1" in
    ''|*[!0-9]*|??????*) return 1 ;;
  esac
  [ "$1" -ge 1 ] && [ "$1" -le 65535 ]
}

prompt_port() {
  while :; do
    prompt_line "监听端口 [2206]: "
    server_port=${prompt_value:-2206}
    if valid_port "$server_port"; then
      server_port=$(printf '%s' "$server_port" | sed 's/^0*//')
      return
    fi
    printf '%s\n' "端口必须是 1-65535 之间的整数。" > /dev/tty
  done
}

prompt_secret() {
  printf '%s' "$1" > /dev/tty
  tty_state=$(stty -g < /dev/tty) || fail "无法读取终端状态"
  stty -echo < /dev/tty
  if ! IFS= read -r prompt_value < /dev/tty; then
    restore_tty
    printf '\n' > /dev/tty
    fail "无法读取密码"
  fi
  restore_tty
  printf '\n' > /dev/tty
}

confirm_purge() {
  (: < /dev/tty) 2>/dev/null || return 0
  prompt_line "即将删除全部配置和数据，确认继续？[y/N]: "
  case "$prompt_value" in
    y|Y) ;;
    *) fail "已取消卸载" ;;
  esac
}

prompt_credentials() {
  [ -r /dev/tty ] && [ -w /dev/tty ] || fail "首次安装必须在交互式终端中运行"
  while :; do
    prompt_line "管理员用户名 [admin]: "
    admin_username=${prompt_value:-admin}
    username_length=$(printf '%s' "$admin_username" | wc -m | tr -d '[:space:]')
    case "$admin_username" in
      *[!A-Za-z0-9_.-]*) printf '%s\n' "用户名只能包含字母、数字、点、下划线和连字符。" > /dev/tty ;;
      *) [ "$username_length" -le 64 ] && break || printf '%s\n' "用户名不能超过 64 个字符。" > /dev/tty ;;
    esac
  done

  while :; do
    prompt_secret "管理员密码（8-128 个字符）: "
    admin_password=$prompt_value
    password_length=$(printf '%s' "$admin_password" | wc -m | tr -d '[:space:]')
    if [ "$password_length" -lt 8 ] || [ "$password_length" -gt 128 ]; then
      printf '%s\n' "密码长度必须在 8-128 个字符之间。" > /dev/tty
      continue
    fi
    prompt_secret "再次输入密码: "
    if [ "$admin_password" = "$prompt_value" ]; then
      break
    fi
    printf '%s\n' "两次输入的密码不一致。" > /dev/tty
  done
}

prompt_database() {
  printf '%s\n' \
    'SQLite：sqlite:///path/to/database.db' \
    'PostgreSQL：postgres://用户:密码@ip:端口/数据库?sslmode=require' > /dev/tty
  while :; do
    prompt_line "数据库 URL [$default_database_url]: "
    database_url=${prompt_value:-$default_database_url}
    case "$database_url" in
      sqlite://*|postgres://*|postgresql://*) break ;;
      *) printf '%s\n' "数据库 URL 格式无效。" > /dev/tty ;;
    esac
  done
}

write_config() {
  escaped_database_url=$(toml_escape "$database_url")
  escaped_username=$(toml_escape "$admin_username")
  escaped_password=$(toml_escape "$admin_password")
  escaped_frontend_dir=$(toml_escape "$public_frontend_dir")
  escaped_admin_dir=$(toml_escape "$admin_frontend_dir")
  escaped_theme_dir=$(toml_escape "$theme_dir")
  config_temp=$(mktemp "$config_dir/.config.toml.XXXXXX")
  {
    printf 'database_url = "%s"\n' "$escaped_database_url"
    printf 'bind_addr = "127.0.0.1:%s"\n' "$server_port"
    printf 'trusted_proxies = ["127.0.0.1/32", "::1/128"]\n'
    printf 'admin_username = "%s"\n' "$escaped_username"
    printf 'admin_password = "%s"\n' "$escaped_password"
    printf 'turnstile_site_key = ""\n'
    printf 'turnstile_secret_key = ""\n'
    printf 'frontend_dir = "%s"\n' "$escaped_frontend_dir"
    printf 'admin_frontend_dir = "%s"\n' "$escaped_admin_dir"
    printf 'theme_dir = "%s"\n' "$escaped_theme_dir"
    printf 'session_ttl_hours = 168\n'
  } > "$config_temp"
  chown root "$config_temp"
  chmod 0600 "$config_temp"
  mv -f "$config_temp" "$config_file"
  config_temp=""
  admin_password=""
  database_url=""
  prompt_value=""
}

download_file() {
  url=$1
  destination=$2
  timeout=$3
  if command -v curl >/dev/null 2>&1; then
    curl --proto '=https' --proto-redir '=https' --tlsv1.2 \
      --fail --location --silent --show-error --max-time "$timeout" \
      "$url" -o "$destination"
  else
    fetch -q -T "$timeout" -o "$destination" "$url"
  fi
}

download_stdout() {
  url=$1
  if command -v curl >/dev/null 2>&1; then
    curl --proto '=https' --proto-redir '=https' --tlsv1.2 \
      --fail --location --silent --show-error --max-time 30 \
      -H 'Accept: application/vnd.github+json' \
      -H 'User-Agent: nodeflare-installer' \
      "$url"
  else
    fetch -q -T 30 -o - "$url"
  fi
}

verify_checksum() {
  case "$platform" in
    linux) actual=$(sha256sum "$archive" | sed 's/[[:space:]].*//') ;;
    macos) actual=$(shasum -a 256 "$archive" | sed 's/[[:space:]].*//') ;;
    freebsd) actual=$(sha256 -q "$archive") ;;
  esac
  [ "$actual" = "$expected" ] || fail "Release SHA-256 校验失败"
}

download_release() {
  case "$platform:$(uname -m)" in
    linux:x86_64|linux:amd64) arch=x64; glibc_loader=/lib64/ld-linux-x86-64.so.2 ;;
    linux:aarch64|linux:arm64) arch=aarch64; glibc_loader=/lib/ld-linux-aarch64.so.1 ;;
    macos:arm64|macos:aarch64) arch=aarch64 ;;
    freebsd:amd64|freebsd:x86_64) arch=x64 ;;
    freebsd:arm64|freebsd:aarch64) arch=aarch64 ;;
    *) fail "暂不支持当前 CPU 架构：$(uname -m)" ;;
  esac

  case "$platform" in
    linux)
      libc=musl
      glibc_version=$(detect_glibc_version || true)
      if [ -n "$glibc_version" ] && glibc_is_supported "$glibc_version"; then
        libc=glibc
      fi
      asset="nodeflare-server-linux-$arch-$libc.tar.gz"
      release_label="$platform $arch $libc"
      ;;
    macos)
      asset=nodeflare-server-macos-aarch64.tar.gz
      release_label="macOS ARM64"
      ;;
    freebsd)
      asset="nodeflare-server-freebsd-$arch.tar.gz"
      release_label="FreeBSD $arch"
      ;;
  esac

  download_dir=$(mktemp -d "${TMPDIR:-/tmp}/nodeflare-install.XXXXXX")
  archive=$download_dir/$asset
  release_api="https://api.github.com/repos/$repository/releases/latest"
  log "获取 latest Release ($release_label)"
  release_json=$(download_stdout "$release_api")
  release_tag=$(printf '%s\n' "$release_json" | tr ',' '\n' | sed -n 's/.*"tag_name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' | sed -n '1p')
  printf '%s\n' "$release_tag" | grep -Eq '^v[0-9]+\.[0-9]+\.[0-9]+$' \
    || fail "GitHub 最新 Release 标签无效"
  expected=$(printf '%s\n' "$release_json" | tr '{' '\n' | awk -v name="$asset" '
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
  [ -n "$expected" ] || fail "Release 缺少 $asset 的 SHA-256 摘要"
  release_base="https://github.com/$repository/releases/download/$release_tag"
  log "下载 $release_tag ($release_label)"
  download_file "$release_base/$asset" "$archive" 120
  verify_checksum

  package_dir=$download_dir/package
  mkdir -p "$package_dir"
  tar -xzf "$archive" -C "$package_dir"
  [ -f "$package_dir/nodeflare" ] \
    && [ -f "$package_dir/share/frontend/index.html" ] \
    && [ -f "$package_dir/share/admin/admin.html" ] \
    && [ -f "$package_dir/LICENSE" ] \
    || fail "Release 文件不完整"
  chmod 0755 "$package_dir/nodeflare"
  binary_version=$("$package_dir/nodeflare" --version) || fail "服务端文件无法运行"
  release_version=${binary_version##* }
  printf '%s\n' "$release_version" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+$' \
    || fail "服务端返回了无效版本号"
  [ "$release_version" = "${release_tag#v}" ] \
    || fail "Release $release_tag 与服务端版本 $release_version 不一致"
}

write_openrc_service() {
  cat > "$openrc_file" <<EOF
#!/sbin/openrc-run
name="NodeFlare"
command="$server_binary"
command_args="--config $config_file"
command_user="root"
supervisor="supervise-daemon"
respawn_delay=5
depend() { need net; }
EOF
  chmod 0755 "$openrc_file"
}

write_launchd_service() {
  cat > "$launchd_file" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>Label</key><string>nodeflare</string>
<key>ProgramArguments</key><array><string>$server_binary</string><string>--config</string><string>$config_file</string></array>
<key>WorkingDirectory</key><string>$config_dir</string>
<key>KeepAlive</key><true/><key>RunAtLoad</key><true/>
<key>StandardOutPath</key><string>/var/log/nodeflare.log</string>
<key>StandardErrorPath</key><string>/var/log/nodeflare.log</string>
</dict></plist>
EOF
  chmod 0600 "$launchd_file"
}

write_freebsd_service() {
  cat > "$freebsd_rc_file" <<EOF
#!/bin/sh
# PROVIDE: nodeflare
# REQUIRE: NETWORKING
# KEYWORD: shutdown
. /etc/rc.subr
name="nodeflare"
rcvar="nodeflare_enable"
pidfile="/var/run/\${name}.pid"
command="/usr/sbin/daemon"
command_args="-P \${pidfile} -r -R 5 -S -T \${name} $server_binary --config $config_file"
load_rc_config "\${name}"
: \${nodeflare_enable:="NO"}
run_rc_command "\$1"
EOF
  chmod 0755 "$freebsd_rc_file"
}

stop_server() {
  case "$init_system" in
    systemd) systemctl stop nodeflare.service >/dev/null 2>&1 || true ;;
    openrc) rc-service nodeflare stop >/dev/null 2>&1 || true ;;
    launchd) launchctl bootout system "$launchd_file" >/dev/null 2>&1 || true ;;
    freebsd) service nodeflare stop >/dev/null 2>&1 || true ;;
  esac
}

service_definition() {
  case "$init_system" in
    systemd) printf '%s' "$systemd_file" ;;
    openrc) printf '%s' "$openrc_file" ;;
    launchd) printf '%s' "$launchd_file" ;;
    freebsd) printf '%s' "$freebsd_rc_file" ;;
  esac
}

snapshot_install() {
  backup_dir=$download_dir/previous
  mkdir -p "$backup_dir"
  if [ -d "$install_dir" ]; then
    cp -Rp "$install_dir" "$backup_dir/install"
    previous_install=true
  fi
  if [ "$share_dir" != "$install_dir/share" ] && [ -d "$share_dir" ]; then
    cp -Rp "$share_dir" "$backup_dir/share"
    previous_share=true
  fi
  service_path=$(service_definition)
  if [ -f "$service_path" ]; then
    cp -p "$service_path" "$backup_dir/service"
    previous_service=true
  fi
  rollback_ready=true
}

rollback_install() {
  rollback_ready=false
  if [ "$previous_install" = true ]; then
    log "安装未完成，正在恢复上一版本"
  else
    log "安装未完成，正在回滚本次更改"
  fi
  stop_server
  rm -rf "$install_dir"
  if [ "$previous_install" = true ]; then
    cp -Rp "$backup_dir/install" "$install_dir"
  fi
  if [ "$share_dir" != "$install_dir/share" ]; then
    rm -rf "$share_dir"
    if [ "$previous_share" = true ]; then
      cp -Rp "$backup_dir/share" "$share_dir"
    fi
  fi
  service_path=$(service_definition)
  rm -f "$service_path"
  if [ "$previous_service" = true ]; then
    cp -p "$backup_dir/service" "$service_path"
  fi
  if [ "$previous_install" = true ] && [ -f "$config_file" ]; then
    if start_server; then
      log "已恢复并重新启动上一版本"
    else
      printf '警告：上一版本已恢复，但服务未能自动启动\n' >&2
    fi
  fi
}

install_service() {
  case "$init_system" in
    systemd) install -m 0644 "$package_dir/nodeflare.service" "$systemd_file" ;;
    openrc) write_openrc_service ;;
    launchd) write_launchd_service ;;
    freebsd) write_freebsd_service ;;
  esac
}

start_server() {
  case "$init_system" in
    systemd)
      systemctl daemon-reload || return 1
      systemctl enable --quiet nodeflare.service >/dev/null || return 1
      systemctl restart nodeflare.service || return 1
      started_pid=$(systemctl show -p MainPID --value nodeflare.service) || return 1
      [ "$started_pid" -gt 0 ] || return 1
      attempt=1
      while [ "$attempt" -le 10 ]; do
        sleep 1
        systemctl is-active --quiet nodeflare.service || return 1
        current_pid=$(systemctl show -p MainPID --value nodeflare.service) || return 1
        [ "$current_pid" = "$started_pid" ] || return 1
        attempt=$((attempt + 1))
      done
      ;;
    openrc)
      rc-update add nodeflare default >/dev/null || return 1
      rc-service nodeflare start || return 1
      rc-service nodeflare status >/dev/null
      ;;
    launchd)
      launchctl bootstrap system "$launchd_file" || return 1
      started_pid=""
      attempt=1
      while [ "$attempt" -le 10 ]; do
        sleep 1
        launchd_status=$(launchctl print system/nodeflare) || return 1
        printf '%s\n' "$launchd_status" | grep -Eq '^[[:space:]]*state = running$' || return 1
        current_pid=$(printf '%s\n' "$launchd_status" | awk '$1 == "pid" && $2 == "=" { print $3; exit }')
        case "$current_pid" in ''|*[!0-9]*|0) return 1 ;; esac
        if [ -z "$started_pid" ]; then
          started_pid=$current_pid
        else
          [ "$current_pid" = "$started_pid" ] || return 1
        fi
        attempt=$((attempt + 1))
      done
      ;;
    freebsd)
      sysrc nodeflare_enable=YES >/dev/null || return 1
      service nodeflare start || return 1
      service nodeflare status >/dev/null
      ;;
  esac
}

status_server() {
  case "$init_system" in
    systemd) systemctl status nodeflare.service --no-pager ;;
    openrc) rc-service nodeflare status ;;
    launchd) launchctl print system/nodeflare ;;
    freebsd) service nodeflare status ;;
  esac
}

restart_server() {
  [ -x "$server_binary" ] && [ -f "$config_file" ] && [ -f "$(service_definition)" ] \
    || fail "未检测到完整安装，请先选择安装 / 更新"
  stop_server
  start_server || fail "服务重启失败，请检查日志"
  log "服务已重启"
}

uninstall_server() {
  purge=$1
  log "停止并移除面板服务"
  stop_server
  case "$init_system" in
    systemd)
      systemctl disable nodeflare.service >/dev/null 2>&1 || true
      ;;
    openrc)
      rc-update del nodeflare default >/dev/null 2>&1 || true
      ;;
    launchd) ;;
    freebsd)
      sysrc -x nodeflare_enable >/dev/null 2>&1 || true
      ;;
  esac
  case "$platform" in
    linux)
      rm -f "$systemd_file" "$openrc_file"
      command -v systemctl >/dev/null 2>&1 && systemctl daemon-reload >/dev/null 2>&1 || true
      ;;
    macos) rm -f "$launchd_file" ;;
    freebsd) rm -f "$freebsd_rc_file" ;;
  esac
  rm -f "$server_binary"
  rm -rf "$share_dir"
  rmdir "$install_dir" 2>/dev/null || true

  if [ "$purge" = true ]; then
    for entry in "$config_dir"/* "$config_dir"/.[!.]* "$config_dir"/..?*; do
      [ -e "$entry" ] || [ -L "$entry" ] || continue
      if [ "$platform" = linux ] && [ "$entry" = "$config_dir/agent" ]; then
        continue
      fi
      rm -rf "$entry"
    done
    rmdir "$config_dir" 2>/dev/null || true
    log "已删除配置和全部服务端持久数据"
  else
    log "已保留配置和服务端持久数据：$config_dir"
  fi
  log "卸载完成"
}

mode=menu
purge=false
case "$#:${1:-}:${2:-}" in
  0::) ;;
  1:--menu:) ;;
  1:-h:|1:--help:) usage; exit 0 ;;
  1:--install:) mode=install ;;
  1:--status:) mode=status ;;
  1:--restart:) mode=restart ;;
  1:--uninstall:) mode=uninstall ;;
  2:--uninstall:--purge) mode=uninstall; purge=true ;;
  *) usage >&2; fail "参数无效" ;;
esac

if [ "$mode" = menu ]; then
  show_menu
fi
[ "$mode" != exit ] || exit 0
[ "$(id -u)" -eq 0 ] || fail "请使用 root 权限运行"
init_system=$(detect_init_system)
if [ "$mode" = uninstall ]; then
  if [ "$purge" = true ]; then
    confirm_purge
  fi
  uninstall_server "$purge"
  exit 0
fi
[ "$init_system" != unknown ] || fail "未检测到受支持的服务管理器"
case "$mode" in
  status) status_server; exit 0 ;;
  restart) restart_server; exit 0 ;;
esac

for required_command in tar install mktemp sed grep wc tr id chown chmod mv cp stty uname cat; do
  command -v "$required_command" >/dev/null 2>&1 || fail "缺少命令：$required_command"
done
case "$platform" in
  linux)
    command -v curl >/dev/null 2>&1 || fail "缺少命令：curl"
    command -v sha256sum >/dev/null 2>&1 || fail "缺少命令：sha256sum"
    ;;
  macos)
    if ! command -v curl >/dev/null 2>&1 || ! command -v shasum >/dev/null 2>&1; then
      fail "缺少 curl 或 shasum"
    fi
    ;;
  freebsd)
    command -v fetch >/dev/null 2>&1 || command -v curl >/dev/null 2>&1 || fail "缺少 fetch 或 curl"
    command -v sha256 >/dev/null 2>&1 || fail "缺少命令：sha256"
    ;;
esac

download_release

new_config=false
if [ ! -f "$config_file" ]; then
  new_config=true
  prompt_credentials
  prompt_port
  prompt_database
fi

if [ "$new_config" = true ]; then
  log "正在安装 v$release_version"
else
  log "正在更新至 v$release_version"
fi
snapshot_install
stop_server
install -d -m 0700 "$config_dir" "$theme_dir"
install -d -m 0755 "$install_dir"
rm -rf "$share_dir"
install -d -m 0755 "$public_frontend_dir" "$admin_frontend_dir"
cp -R "$package_dir/share/frontend/." "$public_frontend_dir/"
cp -R "$package_dir/share/admin/." "$admin_frontend_dir/"
install -m 0755 "$package_dir/nodeflare" "$server_binary"
install_service

if [ "$new_config" = true ]; then
  write_config
else
  chown root "$config_file"
  chmod 0600 "$config_file"
fi

log "正在启动服务"
if ! start_server; then
  if [ "$init_system" = systemd ]; then
    journalctl -u nodeflare.service -n 30 --no-pager >&2 || true
  fi
  fail "服务启动失败，请检查配置中的 bind_addr 是否被占用及数据库连接"
fi
rollback_ready=false

print_install_result
