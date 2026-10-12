#!/usr/bin/env bash
# Checks the openside patch series against a new Chromium pin without modifying it.
#
# Usage: rebase.sh NEW_SRC_DIR
#
# Every file the series touches is copied from NEW_SRC_DIR into a scratch tree, then the
# patches are applied there in series order. Each patch is reported as:
#   clean     applies exactly (no offset, no fuzz)
#   offset    applies with shifted line numbers; refresh recommended
#   fuzz      applies only with fuzz; review and refresh required
#   conflict  does not apply; .rej hunks are listed for manual porting
# Later patches are still tried after a conflict, against the partially applied scratch
# tree, so one run lists every patch that needs work. Exits 1 if any patch needs work.
set -euo pipefail

die() {
    printf 'rebase.sh: %s\n' "$*" >&2
    exit 2
}

[[ $# -eq 1 ]] || {
    printf 'Usage: %s NEW_SRC_DIR\n' "$0" >&2
    exit 2
}
OVERLAY_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
NEW_SRC="$(cd -- "$1" && pwd -P)"
[[ -f "${NEW_SRC}/chrome/VERSION" ]] || die "not a Chromium source tree: ${NEW_SRC}"
command -v patch >/dev/null || die 'missing tool: patch'

declare -A version_parts=()
while IFS='=' read -r key value; do
    [[ -n "$key" ]] && version_parts[$key]="$value"
done <"${NEW_SRC}/chrome/VERSION"
new_version="${version_parts[MAJOR]:-}.${version_parts[MINOR]:-}.${version_parts[BUILD]:-}.${version_parts[PATCH]:-}"
printf 'Pinned: %s  Candidate: %s\n' "$(<"${OVERLAY_DIR}/CHROMIUM_VERSION")" "$new_version"

patches=()
while IFS= read -r entry || [[ -n "$entry" ]]; do
    [[ -z "$entry" || "$entry" == \#* ]] && continue
    [[ -f "${OVERLAY_DIR}/patches/${entry}" ]] || die "missing series entry: ${entry}"
    patches+=("$entry")
done <"${OVERLAY_DIR}/patches/series"

SCRATCH="$(mktemp -d "${TMPDIR:-/tmp}/openside-rebase.XXXXXX")"
trap 'rm -rf -- "$SCRATCH"' EXIT

missing=0
for name in "${patches[@]}"; do
    while IFS= read -r line; do
        file="${line#+++ b/}"
        file="${file%%$'\t'*}"
        [[ -e "${SCRATCH}/${file}" ]] && continue
        mkdir -p -- "$(dirname -- "${SCRATCH}/${file}")"
        if [[ -f "${NEW_SRC}/${file}" ]]; then
            cp -- "${NEW_SRC}/${file}" "${SCRATCH}/${file}"
        else
            printf '%s: %s is missing from the candidate tree\n' "$name" "$file"
            missing=1
        fi
    done < <(grep '^+++ b/' "${OVERLAY_DIR}/patches/${name}")
done

needs_work=$missing
for name in "${patches[@]}"; do
    file="${OVERLAY_DIR}/patches/${name}"
    if output="$(patch --batch --forward --fuzz=0 -d "$SCRATCH" -p1 --dry-run -i "$file" 2>&1)"; then
        patch --batch --forward --fuzz=0 -d "$SCRATCH" -p1 -i "$file" >/dev/null
        if grep -q 'with fuzz\|offset' <<<"$output"; then
            printf 'offset    %s\n' "$name"
            needs_work=1
        else
            printf 'clean     %s\n' "$name"
        fi
    elif patch --batch --forward --fuzz=2 -d "$SCRATCH" -p1 --dry-run -i "$file" >/dev/null 2>&1; then
        patch --batch --forward --fuzz=2 -d "$SCRATCH" -p1 -i "$file" >/dev/null
        printf 'fuzz      %s\n' "$name"
        needs_work=1
    else
        printf 'conflict  %s\n' "$name"
        # Rejects land in the scratch tree, which is removed on exit; list the hunks instead.
        patch --batch --forward --fuzz=0 -d "$SCRATCH" -p1 --no-backup-if-mismatch -i "$file" 2>&1 |
            grep -E '^patching file|FAILED|offset|fuzz|No file to patch' | sed 's/^/          /' || true
        needs_work=1
    fi
done

if [[ $needs_work -ne 0 ]]; then
    printf 'Series needs work for %s; the candidate tree was not modified.\n' "$new_version"
    exit 1
fi
printf 'Series applies cleanly to %s; update browser/CHROMIUM_VERSION when adopting it.\n' "$new_version"
