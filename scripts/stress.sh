#!/usr/bin/env bash
set -euo pipefail

iterations=${LIMITR_STRESS_ITERATIONS:-10}

case "$iterations" in
  '' | *[!0-9]* | 0)
    echo "LIMITR_STRESS_ITERATIONS must be a positive integer" >&2
    exit 2
    ;;
esac

cargo fmt --check
cargo check --all-targets --all-features
cargo clippy --all-targets --all-features -- -D warnings

for iteration in $(seq 1 "$iterations"); do
  cargo test --all-targets --quiet
  bash tests/install.sh >/dev/null
  printf 'stress iteration %s/%s passed\n' "$iteration" "$iterations"
done
