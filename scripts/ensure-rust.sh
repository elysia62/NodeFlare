#!/bin/sh
set -eu

rust_bin_dir="${CARGO_HOME:-$HOME/.cargo}/bin"
case ":$PATH:" in
  *":$rust_bin_dir:"*) ;;
  *) PATH="$rust_bin_dir:$PATH"; export PATH ;;
esac

rust_ready() {
  command -v cargo >/dev/null 2>&1 || return 1
  command -v rustc >/dev/null 2>&1
}

if ! rust_ready; then
  if ! command -v rustup >/dev/null 2>&1; then
    rustup_install_url="https://sh.rustup.rs"
    rustup_installer=$(mktemp)
    trap 'rm -f "$rustup_installer"' 0 HUP INT TERM
    echo "Rust toolchain not found; installing stable Rust..."
    if command -v curl >/dev/null 2>&1; then
      curl --proto '=https' --tlsv1.2 -sSf "$rustup_install_url" -o "$rustup_installer"
    elif command -v wget >/dev/null 2>&1; then
      wget -qO "$rustup_installer" "$rustup_install_url"
    else
      echo "Rust is missing and curl/wget is unavailable" >&2
      exit 1
    fi
    sh "$rustup_installer" -y --profile minimal --default-toolchain stable
    rm -f "$rustup_installer"
    trap - 0 HUP INT TERM
  fi

  if ! command -v cargo >/dev/null 2>&1; then
    rustup toolchain install stable --profile minimal
    rustup default stable
  fi
fi

if ! rust_ready; then
  echo "Required Rust toolchain is unavailable" >&2
  exit 1
fi
