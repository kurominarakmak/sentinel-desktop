#!/bin/sh
# B2-B0 static-fixture integrity verifier only. It does not parse diffs or run Git.
set -eu
export LC_ALL=C
export LANG=C
root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
manifest="$root/docs/fixtures/phase-3c-b2-b0/manifest.tsv"
raw_manifest="$root/docs/fixtures/phase-3c-b2-b0/raw-manifest.tsv"
seen_ids=$(mktemp)
seen_paths=$(mktemp)
actual_paths=$(mktemp)
trap 'rm -f "$seen_ids" "$seen_paths" "$actual_paths"' EXIT
awk -F '\t' '
  $1 !~ /^#/ {
    if (NF != 8 || $1 == "" || $2 == "" || $3 !~ /^[0-9a-f]{64}$/ ||
        ($4 != "positive" && $4 != "negative")) {
      print "invalid manifest row: " NR > "/dev/stderr"
      exit 1
    }
  }
' "$manifest"
awk -F '\t' '
  $1 !~ /^#/ {
    if (NF != 9 || $1 == "" || $2 == "" || $3 !~ /^[0-9a-f]{64}$/ ||
        $4 !~ /^[0-9]+$/ || ($5 != "positive" && $5 != "negative") ||
        $8 !~ /^-?[0-9]+$/) {
      print "invalid raw manifest row: " NR > "/dev/stderr"
      exit 1
    }
  }
' "$raw_manifest"
awk -F '\t' 'NF == 8 && $1 !~ /^#/ { print $1 }' "$manifest" | sort > "$seen_ids"
awk -F '\t' 'NF == 8 && $1 !~ /^#/ { print $2 }' "$manifest" | sort > "$seen_paths"
awk -F '\t' 'NF == 9 && $1 !~ /^#/ { print $1 }' "$raw_manifest" >> "$seen_ids"
awk -F '\t' 'NF == 9 && $1 !~ /^#/ { print $2 }' "$raw_manifest" >> "$seen_paths"
sort -o "$seen_ids" "$seen_ids"
sort -o "$seen_paths" "$seen_paths"
[ "$(uniq -d "$seen_ids" | wc -l | tr -d ' ')" = 0 ]
[ "$(uniq -d "$seen_paths" | wc -l | tr -d ' ')" = 0 ]
[ "$(awk -F '\t' '$1 !~ /^#/ && $4 == "positive" { n++ } END { print n+0 }' "$manifest")" = 13 ]
[ "$(awk -F '\t' '$1 !~ /^#/ && $4 == "negative" { n++ } END { print n+0 }' "$manifest")" = 22 ]
[ "$(awk -F '\t' '$1 !~ /^#/ && $5 == "positive" { n++ } END { print n+0 }' "$raw_manifest")" = 1 ]
[ "$(awk -F '\t' '$1 !~ /^#/ && $5 == "negative" { n++ } END { print n+0 }' "$raw_manifest")" = 43 ]
find "$root/docs/fixtures/phase-3c-b2-b0/positive" "$root/docs/fixtures/phase-3c-b2-b0/negative" "$root/docs/fixtures/phase-3c-b2-b0/raw" -type f -print | sed "s#^$root/docs/fixtures/phase-3c-b2-b0/##" | sort > "$actual_paths"
cmp -s "$seen_paths" "$actual_paths"
awk -F '\t' -v root="$root/docs/fixtures/phase-3c-b2-b0" '
  $1 !~ /^#/ {
    path=root "/" $2;
    cmd="shasum -a 256 \"" path "\"";
    cmd | getline line; close(cmd);
    split(line, pieces, " ");
    if (pieces[1] != $3) { print "hash mismatch: " $1 > "/dev/stderr"; exit 1 }
  }
' "$manifest"
awk -F '\t' -v root="$root/docs/fixtures/phase-3c-b2-b0" '
  $1 !~ /^#/ {
    path=root "/" $2;
    cmd="shasum -a 256 \"" path "\"";
    cmd | getline line; close(cmd);
    split(line, pieces, " ");
    if (pieces[1] != $3) { print "hash mismatch: " $1 > "/dev/stderr"; exit 1 }
    cmd="wc -c < \"" path "\"";
    cmd | getline bytes; close(cmd);
    gsub(/[[:space:]]/, "", bytes);
    if (bytes != $4) { print "length mismatch: " $1 > "/dev/stderr"; exit 1 }
  }
' "$raw_manifest"
printf '%s\n' 'B2-B0 fixture hashes verified.'
