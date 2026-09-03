#!/bin/sh
set -eu

case "${1:-}" in
  frontend)
    paths="frontend package.json bun.lock scripts/build-frontend.sh scripts/build-key.sh scripts/resolve-version.sh"
    ;;
  backend)
    paths="backend/Cargo.toml backend/Cargo.lock backend/src backend/migrations scripts/build-backend.sh scripts/build-key.sh scripts/resolve-version.sh"
    ;;
  *)
    echo "usage: $0 frontend|backend" >&2
    exit 2
    ;;
esac

{
  sh scripts/resolve-version.sh
  git rev-parse HEAD
  git diff --binary -- $paths
  git ls-files --others --exclude-standard -- $paths | while IFS= read -r file; do
    printf '%s ' "$file"
    git hash-object "$file"
  done
} | cksum | awk '{ print $1 "-" $2 }'
