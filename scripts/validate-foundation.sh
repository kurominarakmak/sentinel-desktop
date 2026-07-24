#!/bin/sh
set -eu

cargo fmt --all --check
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
./scripts/verify-core-boundaries.sh

cd apps/desktop
npm ci
npm test
npm run build
