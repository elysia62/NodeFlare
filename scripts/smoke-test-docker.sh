#!/bin/sh
set -eu

image=${1:-nodeflare:smoke}
: "${NODEFLARE_VERSION:?Set NODEFLARE_VERSION to the expected server version}"
script_dir=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd)
root_dir=$(CDPATH='' cd -- "$script_dir/.." && pwd)
volume=""
container=""
cleanup() {
  if [ -n "$container" ]; then
    docker logs "$container" 2>&1 || true
    docker rm --force "$container" >/dev/null || true
  fi
  if [ -n "$volume" ]; then docker volume rm "$volume" >/dev/null || true; fi
}
trap cleanup EXIT
trap 'exit 1' HUP INT TERM

version_output=$(docker run --rm "$image" --version)
[ "${version_output##* }" = "$NODEFLARE_VERSION" ] || {
  echo "Unexpected server version: $version_output" >&2
  exit 1
}
volume=$(docker volume create)
docker run --rm --user 0:0 --entrypoint sh \
  --mount "type=volume,src=$volume,dst=/etc/nodeflare" \
  --mount "type=bind,src=$root_dir/docker/config.example.toml,dst=/tmp/config.example.toml,readonly" \
  "$image" -c '
    set -eu
    sed "s/CHANGE_ME_WITH_A_STRONG_PASSWORD/DockerSmokePassword123/" /tmp/config.example.toml > /etc/nodeflare/config.toml
    chown -R 10001:10001 /etc/nodeflare
    chmod 700 /etc/nodeflare
    chmod 600 /etc/nodeflare/config.toml
  '
container=$(docker run --detach \
  --publish 127.0.0.1::2206 \
  --mount "type=volume,src=$volume,dst=/etc/nodeflare" \
  "$image")
address=$(docker port "$container" 2206/tcp)
base="http://$address"
curl --noproxy '*' --fail --silent --show-error --retry 30 --retry-connrefused --retry-delay 1 --max-time 3 \
  "$base/api/bootstrap" >/dev/null
for path in / /admin/login; do
  html=$(curl --noproxy '*' --fail --silent --show-error --max-time 10 "$base$path")
  script=$(printf '%s\n' "$html" | sed -n 's/.*src="\([^"]*\.js\)".*/\1/p')
  stylesheet=$(printf '%s\n' "$html" | sed -n 's/.*href="\([^"]*\.css\)".*/\1/p')
  [ -n "$script" ] && [ -n "$stylesheet" ]
  curl --noproxy '*' --fail --silent --show-error --max-time 10 "$base$script" >/dev/null
  curl --noproxy '*' --fail --silent --show-error --max-time 10 "$base$stylesheet" >/dev/null
done
docker exec "$container" sh -c '
  test -s /etc/nodeflare/nodeflare.db
  ! grep -q DockerSmokePassword123 /etc/nodeflare/config.toml
'
printf '%s\n' 'Docker image smoke test passed'
