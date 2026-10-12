#!/bin/sh
# Run every check the CI quality job runs, so a push is not the first time the
# formatting, lint and test steps meet this code.
#
# Steps that need a service CI provides are skipped with a notice instead of
# failing: PostgreSQL tests only run when NODEFLARE_TEST_POSTGRES_URL is set.
set -eu

root_dir=$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)
cd "$root_dir"

failures=0

step() {
  printf '\n▶ %s\n' "$1"
  shift
  if "$@"; then
    return 0
  fi
  failures=$((failures + 1))
  printf '  ✗ 上一步失败: %s\n' "$*"
}

# Frontend
step "类型检查（前端）" bun run --cwd frontend tsc -b
step "测试（前端）" bun test --cwd frontend
step "构建（前端）" bun run build:frontend

# Shell 与 PowerShell 语法
step "语法检查（shell）" sh -c '
  for script in install.sh agent/agent.sh agent/install-macos.sh agent/install-freebsd.sh scripts/*.sh start.sh dev.sh; do
    sh -n "$script" || exit "$?"
  done
'

# Rust：格式、lint、测试
for crate in backend agent shared; do
  step "格式检查（$crate）" cargo fmt --manifest-path "$crate/Cargo.toml" -- --check
done
step "lint（后端）" env CARGO_TARGET_DIR=target/backend cargo clippy --locked --manifest-path backend/Cargo.toml --all-targets -- -D warnings
step "lint（agent）" env CARGO_TARGET_DIR=target/agent cargo clippy --locked --manifest-path agent/Cargo.toml --all-targets -- -D warnings
step "测试（telemetry）" env CARGO_TARGET_DIR=target/backend cargo test --locked --manifest-path shared/Cargo.toml
step "测试（后端）" env CARGO_TARGET_DIR=target/backend cargo test --locked --manifest-path backend/Cargo.toml
step "测试（agent）" env CARGO_TARGET_DIR=target/agent cargo test --locked --manifest-path agent/Cargo.toml

if [ -n "${NODEFLARE_TEST_POSTGRES_URL:-}" ]; then
  step "测试（PostgreSQL）" env CARGO_TARGET_DIR=target/backend cargo test --locked --manifest-path backend/Cargo.toml postgres_ -- --ignored
else
  printf '\n▶ 测试（PostgreSQL）\n  已跳过：设置 NODEFLARE_TEST_POSTGRES_URL 后重跑\n'
fi

printf '\n%s\n' "────────────────────────────────────────"
if [ "$failures" -eq 0 ]; then
  printf '✅ 全部检查通过\n'
else
  printf '❌ %s 项检查失败\n' "$failures"
fi
exit "$failures"
