#!/bin/sh
set -eu

if grep -R -n -E '^[[:space:]]*tauri([[:space:]]|=)' crates/*/Cargo.toml; then
  echo "Reusable crates must not depend on Tauri." >&2
  exit 1
fi

echo "Core dependency boundary verified: no crate under crates/ depends on Tauri."
