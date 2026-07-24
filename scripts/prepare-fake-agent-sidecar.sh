#!/usr/bin/env bash
set -euo pipefail

task_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
profile=""
requested_target=""
dry_run=false

usage() {
  echo "usage: $0 --profile debug|release [--target <triple>] [--dry-run]" >&2
  exit 2
}
while (($#)); do
  case "$1" in
    --profile) (($# >= 2)) || usage; profile="$2"; shift 2 ;;
    --target) (($# >= 2)) || usage; requested_target="$2"; shift 2 ;;
    --target=*) requested_target="${1#--target=}"; shift ;;
    --dry-run) dry_run=true; shift ;;
    *) usage ;;
  esac
done
[[ "$profile" == debug || "$profile" == release ]] || usage
if [[ -n "$requested_target" && ! "$requested_target" =~ ^[A-Za-z0-9._-]+$ ]]; then
  echo "invalid Rust target triple" >&2; exit 2
fi

host=$(rustc -vV | sed -n 's/^host: //p')
[[ -n "$host" ]]
target="${requested_target:-$host}"
suffix=""
[[ "$target" == *windows* ]] && suffix=".exe"
target_dir="${CARGO_TARGET_DIR:-$task_root/target}"
[[ "$target_dir" = /* ]] || target_dir="$task_root/$target_dir"
profile_dir="$target_dir"
[[ -n "$requested_target" ]] && profile_dir="$profile_dir/$target"
profile_dir="$profile_dir/$profile"
artifact="$profile_dir/sentinel-fake-agent$suffix"
external="$task_root/apps/desktop/src-tauri/binaries/sentinel-fake-agent-$target$suffix"

printf 'sidecar profile: %s\nsidecar target: %s\ncargo artifact: %s\nexternalBin source: %s\nruntime sibling: %s\n' \
  "$profile" "$target" "$artifact" "$external" "$artifact"
if "$dry_run"; then exit 0; fi

cd "$task_root"
cargo_args=(build --manifest-path "$task_root/Cargo.toml" -p sentinel-fake-agent)
[[ "$profile" == release ]] && cargo_args+=(--release)
[[ -n "$requested_target" ]] && cargo_args+=(--target "$target")
cargo "${cargo_args[@]}"
[[ -s "$artifact" ]] || { echo "fake-agent artifact was not produced" >&2; exit 1; }
mkdir -p "$(dirname "$external")"
temporary=$(mktemp "${external}.tmp.XXXXXX")
trap 'rm -f "$temporary"' EXIT
cp "$artifact" "$temporary"
chmod u+x "$temporary" 2>/dev/null || true
mv -f "$temporary" "$external"
trap - EXIT
