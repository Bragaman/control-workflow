#!/bin/sh
# Build the current checkout and delegate installation to its native installer.
set -eu
if [ "$#" -ne 2 ] || [ "$1" != "--prefix" ] || [ -z "$2" ]; then
    echo 'Usage: scripts/install.sh --prefix PATH' >&2
    exit 2
fi
case "$2" in
    /*) prefix=$2 ;;
    *) prefix=$PWD/$2 ;;
esac
root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd -P)
cd "$root"
command -v cargo >/dev/null 2>&1 || { echo 'cargo is required' >&2; exit 1; }
command -v rustc >/dev/null 2>&1 || { echo 'rustc is required' >&2; exit 1; }
host=$(rustc -vV | sed -n 's/^host: //p')
[ -n "$host" ] || { echo 'Cannot determine Rust host target' >&2; exit 1; }
cargo build --locked --release --target "$host" --target-dir "$root/target"
exec "$root/target/$host/release/control-workflow" install --prefix "$prefix"
