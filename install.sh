#!/bin/sh
set -eu

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
config_dir=/etc/nodeflare
config_file=$config_dir/config.toml
state_dir=/var/lib/nodeflare
install_dir=/opt/nodeflare
share_dir=$install_dir/share
public_frontend_dir=$share_dir/frontend
admin_frontend_dir=$share_dir/admin
agent_installer_dir=$share_dir/agent
server_binary=$install_dir/nodeflare
service_name=nodeflare
service_file=/etc/systemd/system/nodeflare.service
legacy_current_binary=/usr/local/bin/nodeflare
legacy_server_binary=/usr/local/bin/nodeflare-server
legacy_share_dir=/usr/local/share/nodeflare
legacy_service_file=/etc/systemd/system/nodeflare-server.service
agent_service_file=/etc/systemd/system/nodeflare-agent.service
build_target_dir=""
config_temp=""
tty_state=""

usage() {
  printf '%s\n' \
    'NodeFlare 面板安装脚本' \
    '' \
    '用法：' \
    '  sudo ./install.sh' \
    '  sudo ./install.sh --uninstall' \
    '  sudo ./install.sh --uninstall --purge' \
    '' \
    '首次安装会询问管理员用户名和密码。' \
    '其他设置写入 /etc/nodeflare/config.toml，可在安装后编辑。' \
    '重复运行会保留已有配置和数据库。' \
    '--uninstall 保留配置和数据；只有同时指定 --purge 才彻底删除。'
}

log() {
  printf '[NodeFlare] %s\n' "$1"
}

fail() {
  printf '[NodeFlare] 错误：%s\n' "$1" >&2
  exit 1
}

restore_tty() {
  if [ -n "$tty_state" ]; then
    stty "$tty_state" < /dev/tty 2>/dev/null || true
    tty_state=""
  fi
}

cleanup() {
  restore_tty
  [ -z "$config_temp" ] || rm -f -- "$config_temp"
  [ -z "$build_target_dir" ] || rm -rf -- "$build_target_dir"
}

trap cleanup EXIT HUP INT TERM

toml_escape() {
  printf '%s' "$1" | sed -e 's/\\/\\\\/g' -e 's/"/\\"/g'
}

prompt_line() {
  printf '%s' "$1" > /dev/tty
  IFS= read -r prompt_value < /dev/tty || fail "无法读取输入"
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

prompt_credentials() {
  [ -r /dev/tty ] && [ -w /dev/tty ] || fail "首次安装必须在交互式终端中运行"
  while :; do
    prompt_line "管理员用户名 [admin]: "
    admin_username=${prompt_value:-admin}
    username_length=$(printf '%s' "$admin_username" | wc -m | tr -d '[:space:]')
    case "$admin_username" in
      *[!A-Za-z0-9_.-]*)
        printf '%s\n' "用户名只能包含字母、数字、点、下划线和连字符。" > /dev/tty
        ;;
      *)
        if [ "$username_length" -gt 64 ]; then
          printf '%s\n' "用户名不能超过 64 个字符。" > /dev/tty
        else
          break
        fi
        ;;
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
    if LC_ALL=C printf '%s' "$admin_password" | grep -q '[[:cntrl:]]'; then
      printf '%s\n' "密码不能包含控制字符。" > /dev/tty
      continue
    fi
    normalized_password=$(printf '%s' "$admin_password" | tr '[:upper:]' '[:lower:]')
    case "$normalized_password" in
      change_me_with_a_strong_password|change-me-with-a-strong-password)
        printf '%s\n' "请不要使用示例占位密码。" > /dev/tty
        continue
        ;;
    esac
    prompt_secret "再次输入密码: "
    [ "$admin_password" = "$prompt_value" ] || {
      printf '%s\n' "两次输入的密码不一致。" > /dev/tty
      continue
    }
    break
  done
}

write_config() {
  escaped_username=$(toml_escape "$admin_username")
  escaped_password=$(toml_escape "$admin_password")
  config_temp=$(mktemp "$config_dir/.config.toml.XXXXXX")
  {
    printf '%s\n' '# NodeFlare 服务端配置'
    printf '%s\n' '# 修改后运行：systemctl restart nodeflare'
    printf '\n'
    printf 'database_url = "sqlite:///var/lib/nodeflare/nodeflare.db"\n'
    printf 'bind_addr = "127.0.0.1:8080"\n'
    printf 'admin_username = "%s"\n' "$escaped_username"
    printf 'admin_password = "%s"\n' "$escaped_password"
    printf '\n'
    printf '%s\n' '# Cloudflare Turnstile：两项留空即禁用'
    printf 'turnstile_site_key = ""\n'
    printf 'turnstile_secret_key = ""\n'
    printf '\n'
    printf 'frontend_dir = "/opt/nodeflare/share/frontend"\n'
    printf 'admin_frontend_dir = "/opt/nodeflare/share/admin"\n'
    printf 'agent_dir = "/opt/nodeflare/share/agent"\n'
    printf 'theme_dir = "/var/lib/nodeflare/themes"\n'
    printf 'session_ttl_hours = 168\n'
  } > "$config_temp"
  chown root:root "$config_temp"
  chmod 0600 "$config_temp"
  mv -f -- "$config_temp" "$config_file"
  config_temp=""
  admin_password=""
  prompt_value=""
}

migrate_config_paths() {
  config_temp=$(mktemp "$config_dir/.config.toml.XXXXXX")
  sed \
    -e 's|^frontend_dir = "/usr/local/share/nodeflare/frontend"$|frontend_dir = "/opt/nodeflare/share/frontend"|' \
    -e 's|^admin_frontend_dir = "/usr/local/share/nodeflare/admin"$|admin_frontend_dir = "/opt/nodeflare/share/admin"|' \
    -e 's|^agent_dir = "/usr/local/share/nodeflare/agent"$|agent_dir = "/opt/nodeflare/share/agent"|' \
    "$config_file" > "$config_temp"
  chown root:root "$config_temp"
  chmod 0600 "$config_temp"
  mv -f -- "$config_temp" "$config_file"
  config_temp=""
}

uninstall_server() {
  purge=$1
  log "停止并移除 NodeFlare 面板服务"
  if command -v systemctl >/dev/null 2>&1; then
    migrate_legacy_agent_service
    systemctl disable --now "$service_name.service" >/dev/null 2>&1 || true
    systemctl disable --now nodeflare-server.service >/dev/null 2>&1 || true
  fi
  rm -f -- "$service_file" "$server_binary" "$legacy_service_file" "$legacy_current_binary" "$legacy_server_binary"
  rm -rf -- "$share_dir"
  rmdir "$install_dir" 2>/dev/null || true
  if command -v systemctl >/dev/null 2>&1; then
    systemctl daemon-reload >/dev/null 2>&1 || true
    systemctl reset-failed "$service_name.service" >/dev/null 2>&1 || true
    systemctl reset-failed nodeflare-server.service >/dev/null 2>&1 || true
  fi

  if [ "$purge" = true ]; then
    rm -rf -- "$config_dir" "$state_dir"
    log "已删除配置和数据库"
  else
    log "已保留配置 $config_file 和数据目录 $state_dir"
    log "如需彻底删除，运行：sudo ./install.sh --uninstall --purge"
  fi
  log "卸载完成"
}

migrate_legacy_agent_service() {
  if [ -f "$service_file" ] && grep -Fq '/opt/nodeflare/agent' "$service_file"; then
    log "将旧 Agent 服务 nodeflare 迁移为 nodeflare-agent"
    systemctl disable --now nodeflare.service >/dev/null 2>&1 || true
    if [ ! -e "$agent_service_file" ]; then
      mv -- "$service_file" "$agent_service_file"
    else
      rm -f -- "$service_file"
    fi
    systemctl daemon-reload
    systemctl enable --now nodeflare-agent.service >/dev/null 2>&1 || true
  fi
}

mode=install
purge=false
case "$#:${1:-}:${2:-}" in
  0::) ;;
  1:-h:|1:--help:) usage; exit 0 ;;
  1:--uninstall:) mode=uninstall ;;
  2:--uninstall:--purge) mode=uninstall; purge=true ;;
  *) usage >&2; fail "参数无效" ;;
esac

[ "$(id -u)" -eq 0 ] || fail "请使用 root 权限运行：sudo ./install.sh"
if [ "$mode" = uninstall ]; then
  uninstall_server "$purge"
  exit 0
fi
[ -d /run/systemd/system ] && command -v systemctl >/dev/null 2>&1 \
  || fail "当前系统未使用 systemd"
migrate_legacy_agent_service
for required_command in bun install mktemp sed grep wc tr id chown chmod mv cp stty; do
  command -v "$required_command" >/dev/null 2>&1 || fail "缺少命令：$required_command"
done

new_config=false
if [ ! -f "$config_file" ]; then
  new_config=true
  prompt_credentials
else
  log "保留已有配置：$config_file"
fi

log "构建前端和服务端"
build_target_dir=$(mktemp -d /tmp/nodeflare-build.XXXXXX)
CARGO_TARGET_DIR=$build_target_dir/cargo
export CARGO_TARGET_DIR
sh "$script_dir/scripts/build-frontend.sh"
sh "$script_dir/scripts/build-backend.sh"

install -d -m 0700 -o root -g root "$config_dir"
install -d -m 0750 -o root -g root "$state_dir"
install -d -m 0750 -o root -g root "$state_dir/themes"
chown -R root:root "$state_dir"
install -d -m 0755 -o root -g root "$install_dir"
install -d -m 0755 -o root -g root "$share_dir"
rm -rf -- "$public_frontend_dir" "$admin_frontend_dir" "$agent_installer_dir"
install -d -m 0755 -o root -g root "$public_frontend_dir" "$admin_frontend_dir" "$agent_installer_dir"
cp -a "$script_dir/frontend/dist/." "$public_frontend_dir/"
cp -a "$script_dir/frontend/admin-dist/." "$admin_frontend_dir/"
install -m 0755 "$script_dir/agent/agent.sh" "$agent_installer_dir/agent.sh"
install -m 0755 "$script_dir/agent/install-macos.sh" "$agent_installer_dir/install-macos.sh"
install -m 0755 "$script_dir/agent/install-freebsd.sh" "$agent_installer_dir/install-freebsd.sh"
install -m 0644 "$script_dir/agent/install.ps1" "$agent_installer_dir/install.ps1"
chown -R root:root "$share_dir"
chmod -R u=rwX,go=rX "$share_dir"
systemctl disable --now nodeflare-server.service >/dev/null 2>&1 || true
rm -f -- "$legacy_service_file" "$legacy_current_binary" "$legacy_server_binary"
install -m 0755 "$CARGO_TARGET_DIR/release/nodeflare" "$server_binary"
install -m 0644 "$script_dir/deploy/nodeflare.service" "$service_file"

if [ "$new_config" = true ]; then
  write_config
else
  migrate_config_paths
  chown root:root "$config_file"
  chmod 0600 "$config_file"
fi

if ! grep -Fq "$legacy_share_dir/" "$config_file"; then
  rm -rf -- "$legacy_share_dir"
fi

systemctl daemon-reload
systemctl enable "$service_name.service" >/dev/null
if ! systemctl restart "$service_name.service"; then
  journalctl -u "$service_name.service" -n 30 --no-pager >&2 || true
  fail "NodeFlare 服务启动失败"
fi
if ! systemctl is-active --quiet "$service_name.service"; then
  journalctl -u "$service_name.service" -n 30 --no-pager >&2 || true
  fail "NodeFlare 服务未进入运行状态"
fi

log "安装完成"
printf '%s\n' \
  "配置文件：$config_file" \
  "程序目录：$install_dir" \
  "数据目录：$state_dir" \
  '服务状态：systemctl status nodeflare' \
  '默认访问：http://127.0.0.1:8080'
