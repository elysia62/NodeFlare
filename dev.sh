#!/bin/sh
set -eu

root_dir=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
cd "$root_dir"

if [ ! -f backend/config.toml ]; then
  echo "配置文件不存在，从示例复制..."
  cp backend/config.example.toml backend/config.toml
  echo "请编辑 backend/config.toml（尤其是 admin_password）后重新运行"
  exit 1
fi

echo "构建前端..."
sh scripts/build-frontend.sh

echo "启动后端（开发模式）..."
exec cargo run --locked --manifest-path backend/Cargo.toml -- --config backend/config.toml
