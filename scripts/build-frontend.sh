#!/bin/sh
set -eu

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
root_dir=$(CDPATH= cd -- "$script_dir/.." && pwd)
NODEFLARE_VERSION=$(sh "$script_dir/resolve-version.sh")
VITE_NODEFLARE_VERSION=$NODEFLARE_VERSION
export VITE_NODEFLARE_VERSION
cd "$root_dir"

[ -d node_modules ] || bun install --frozen-lockfile
bun run --cwd frontend build
