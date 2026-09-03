#!/bin/sh
set -eu

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
root_dir=$(CDPATH= cd -- "$script_dir/.." && pwd)
NODEFLARE_VERSION=$(sh "$script_dir/resolve-version.sh")
export NODEFLARE_VERSION
cd "$root_dir"
. "$script_dir/ensure-rust.sh"

agent_target_dir=${CARGO_TARGET_DIR:-target/agent}
CARGO_TARGET_DIR="$agent_target_dir" cargo build --manifest-path agent/Cargo.toml --release --locked
