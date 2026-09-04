#!/bin/sh
set -eu

binary=$1
maximum=$2
versions=$(LC_ALL=C readelf -W --version-info "$binary" \
  | sed -n 's/.*Name: GLIBC_\([0-9][0-9.]*\).*/\1/p')

[ -n "$versions" ] || {
  printf 'No GLIBC symbols found in %s\n' "$binary" >&2
  exit 1
}

too_new=$(printf '%s\n' "$versions" | awk -v maximum="$maximum" '
  function newer(version, limit, current, expected, count, i) {
    count = split(version, current, ".")
    split(limit, expected, ".")
    for (i = 1; i <= count || i <= 3; i++) {
      if (current[i] + 0 > expected[i] + 0) return 1
      if (current[i] + 0 < expected[i] + 0) return 0
    }
    return 0
  }
  newer($0, maximum) { print; exit }
')

if [ -n "$too_new" ]; then
  printf '%s requires GLIBC_%s, newer than GLIBC_%s\n' "$binary" "$too_new" "$maximum" >&2
  exit 1
fi

highest=$(printf '%s\n' "$versions" | sort -V | tail -n 1)
printf '%s requires at most GLIBC_%s\n' "$binary" "$highest"
