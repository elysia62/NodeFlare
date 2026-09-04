#!/bin/sh
set -eu

script_dir=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
root_dir=$(CDPATH='' cd -- "$script_dir/.." && pwd)
NODEFLARE_VERSION=$(sh "$script_dir/resolve-version.sh")
export NODEFLARE_VERSION
cd "$root_dir"
. "$script_dir/ensure-rust.sh"

cargo build --manifest-path backend/Cargo.toml --release --locked
