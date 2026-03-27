#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUTPUT_PATH="${1:-$ROOT_DIR/openapi/fluxa-openapi.json}"

cd "$ROOT_DIR"
cargo run --quiet --bin generate_openapi -- "$OUTPUT_PATH"
