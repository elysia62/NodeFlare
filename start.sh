#!/bin/sh
set -eu

root_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
cd "$root_dir"

config_path=${NODEFLARE_CONFIG:-}
if [ -z "$config_path" ]; then
  if [ -f /etc/nodeflare/config.toml ]; then
    config_path=/etc/nodeflare/config.toml
  elif [ -f backend/config.toml ]; then
    config_path=backend/config.toml
    echo "正在使用兼容的本地配置 backend/config.toml"
  else
    echo "未找到 /etc/nodeflare/config.toml" >&2
    echo "请先运行 sudo ./install.sh，安装时会询问管理员用户名和密码。" >&2
    exit 1
  fi
fi

echo "构建前端..."
sh scripts/build-frontend.sh

echo "构建后端..."
sh scripts/build-backend.sh

echo "启动后端..."
exec backend/target/release/nodeflare --config "$config_path"
