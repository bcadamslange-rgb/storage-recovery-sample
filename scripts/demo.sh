#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
cargo build --release --locked --offline
python3 scripts/demo.py --binary target/release/kvrecover
