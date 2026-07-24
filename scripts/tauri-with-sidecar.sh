#!/usr/bin/env bash
set -euo pipefail

task_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
mode="${1:-}"
case "$mode" in
  dev) profile=debug ;;
  build) profile=release ;;
  *) echo "expected Tauri dev or build command" >&2; exit 2 ;;
esac

target=""
args=("$@")
for ((i = 0; i < ${#args[@]}; i++)); do
  case "${args[i]}" in
    --target)
      ((i + 1 < ${#args[@]})) || { echo "--target requires a triple" >&2; exit 2; }
      target="${args[i + 1]}"; ((i++)) ;;
    --target=*) target="${args[i]#--target=}" ;;
  esac
done
prepare=("$task_root/scripts/prepare-fake-agent-sidecar.sh" --profile "$profile")
[[ -n "$target" ]] && prepare+=(--target "$target")
"${prepare[@]}"
tauri "${args[@]}"
