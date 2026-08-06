#!/usr/bin/env sh

# Read-only preflight for the local V3 OSS research checkout. This script never
# modifies the resolved directory or invokes commands that can modify it.
set -eu

repository_dir=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd -P)
local_config="$repository_dir/.sentinel/local.toml"

if [ -n "${SENTINEL_V3_OSS_DIR:-}" ]; then
  oss_research_dir=$SENTINEL_V3_OSS_DIR
  resolution_source='SENTINEL_V3_OSS_DIR'
elif [ -f "$local_config" ]; then
  oss_research_dir=$(sed -n 's/^oss_research_dir[[:space:]]*=[[:space:]]*"\(.*\)"[[:space:]]*$/\1/p' "$local_config")
  resolution_source='.sentinel/local.toml'
else
  oss_research_dir="$repository_dir/../sentinel-v3-oss"
  resolution_source='../sentinel-v3-oss fallback'
fi

if [ -z "$oss_research_dir" ]; then
  echo "error: no oss_research_dir configured in $local_config" >&2
  exit 1
fi

oss_research_dir=$(CDPATH= cd -- "$oss_research_dir" 2>/dev/null && pwd -P) || {
  echo "error: OSS research directory does not exist: $oss_research_dir" >&2
  exit 1
}

if [ "$oss_research_dir" = "$repository_dir" ] || [ "${oss_research_dir#"$repository_dir"/}" != "$oss_research_dir" ]; then
  echo "error: OSS research directory must be outside this Sentinel repository" >&2
  exit 1
fi

expected_repositories='OpenCovibe agent-client-protocol agent-sessions agetor architect-loop cctop codex-plugin-cc codex-review dmux elves jean parallel-code ralph-review threadline tuicommander'
missing_repositories=''
detected_repositories=''
for donor in $expected_repositories; do
  if [ -d "$oss_research_dir/$donor" ]; then
    detected_repositories="${detected_repositories}${detected_repositories:+, }$donor"
  else
    missing_repositories="${missing_repositories}${missing_repositories:+, }$donor"
  fi
done

if [ -n "$missing_repositories" ]; then
  echo "error: missing expected donor repositories: $missing_repositories" >&2
  exit 1
fi

printf '%s\n' "Resolved OSS directory: $oss_research_dir" \
  "Resolution source: $resolution_source" \
  "Repositories detected: $detected_repositories" \
  'Access mode: read-only (validation performs no writes)'
