#!/usr/bin/env bash
# Portability gate: machine-specific literals outside excluded/allowlisted locations fail the build.
set -euo pipefail
cd "$(dirname "$0")/.."
pattern='/home/|/Users/|sharosoo|workspaces/|\.omp/|Hyprland|127\.0\.0\.1:8765'
excluded='^docs/(research|spikes)/'
allow=$(grep -vE '^\s*(#|$)' docs/PORTABILITY-ALLOWLIST.txt | sed -E 's/\s+#.*$//')
hits=$(grep -rnE "$pattern" crates agent/src agent/tests browser tools docs -I | grep -vE "$excluded" || true)
remaining=$(while IFS= read -r hit; do
  [ -z "$hit" ] && continue
  file=${hit%%:*}; rest=${hit#*:}; line=${rest%%:*}
  grep -qxF -e "$file:$line" -e "$file:*" <<<"$allow" || printf '%s\n' "$hit"
done <<<"$hits")
if [ -n "$remaining" ]; then
  printf 'portability gate: machine-specific references found:\n%s\n' "$remaining" >&2
  exit 1
fi
echo "portability gate: 0 hits"
