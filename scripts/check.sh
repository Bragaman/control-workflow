#!/bin/sh
# Run the same source, release and contract checks locally and on CI.
set -eu
cd "$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"

cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
python3 -m ruff check
cargo test --locked
cargo build --locked --release
python3 scripts/release_audit.py
CONTROL_WORKFLOW_BIN="$PWD/target/release/control-workflow" PYTHONDONTWRITEBYTECODE=1 \
    python3 -m unittest discover -s tests -v
