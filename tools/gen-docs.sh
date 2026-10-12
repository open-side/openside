#!/usr/bin/env bash
# Regenerates docs/reference/{cli,api}.md, or with --check fails when the committed files differ.
#   cli.md  <- clap definitions  (cargo run -p openside-cli --example cli-docs)
#   api.md  <- docs/specs/openapi.json (tools/gen-api-docs.py)
set -euo pipefail
cd "$(dirname "$0")/.."

mode=write
case "${1:-}" in
  "") ;;
  --check) mode=check ;;
  *) echo "usage: $0 [--check]" >&2; exit 2 ;;
esac

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
mkdir -p docs/reference

cargo run -q -j "${OPENSIDE_GATE_JOBS:-8}" -p openside-cli --example cli-docs >"$tmp/cli.md"
python3 tools/gen-api-docs.py >"$tmp/api.md"

status=0
for name in cli api; do
  target=docs/reference/$name.md
  if [ "$mode" = check ]; then
    if ! diff -u "$target" "$tmp/$name.md"; then
      echo "$target is stale: run tools/gen-docs.sh" >&2
      status=1
    fi
  else
    cp "$tmp/$name.md" "$target"
    echo "wrote $target"
  fi
done
exit "$status"
