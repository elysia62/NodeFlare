#!/bin/sh
set -eu

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
root_dir=$(CDPATH= cd -- "$script_dir/.." && pwd)

raw=${NODEFLARE_VERSION:-}
if [ -z "$raw" ]; then
  raw=$(git -C "$root_dir" describe --tags --exact-match --match 'v[0-9]*.[0-9]*.[0-9]*' 2>/dev/null || true)
fi
if [ -z "$raw" ]; then
  raw=$(git -C "$root_dir" describe --tags --abbrev=0 --match 'v[0-9]*.[0-9]*.[0-9]*' 2>/dev/null || true)
fi
if [ -z "$raw" ]; then
  raw=$(sed -n 's/^[[:space:]]*"version": "\([^"]*\)",*$/\1/p' "$root_dir/package.json" | sed -n '1p')
fi

version=${raw#v}
if ! printf '%s\n' "$version" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+$'; then
  echo "invalid NodeFlare version: $raw" >&2
  exit 1
fi
printf '%s\n' "$version"
