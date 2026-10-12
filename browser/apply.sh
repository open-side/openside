#!/usr/bin/env bash
# Applies the openside overlay (browser/src + browser/patches/series) to a
# pinned, domain-substituted Chromium tree.
#
# Re-runnable: patches already applied with identical content are skipped. A
# changed or removed patch is reverse-applied, together with every later
# patch, from the copy recorded when it was applied; the series is then
# re-applied in order. Application stops at the first patch that does not
# apply cleanly, leaving the tree and the recorded state consistent.
set -euo pipefail

die() {
    printf 'apply.sh: %s\n' "$*" >&2
    exit 1
}

if [[ $# -gt 1 ]]; then
    printf 'Usage: %s [SRC_DIR]\n' "$0" >&2
    exit 2
fi
OVERLAY_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
if [[ $# -eq 1 ]]; then
    SRC_DIR="$1"
else
    : "${OPENSIDE_BROWSER_BUILD_DIR:?set OPENSIDE_BROWSER_BUILD_DIR or pass SRC_DIR}"
    SRC_DIR="${OPENSIDE_BROWSER_BUILD_DIR}/portablelinux/build/src"
fi
SRC_DIR="$(cd -- "$SRC_DIR" && pwd -P)"
STATE="${SRC_DIR}/.openside-overlay"
LEGACY_STAMP="${SRC_DIR}/.openside-overlay.stamp"

for tool in patch sha256sum flock cp cmp; do
    command -v "$tool" >/dev/null || die "missing tool: $tool"
done
[[ -f "${SRC_DIR}/chrome/browser/BUILD.gn" && -f "${SRC_DIR}/.domsub.stamp" ]] ||
    die 'expected a fully patched/domain-substituted portablelinux source tree'
[[ -f "${SRC_DIR}/chrome/VERSION" ]] || die "missing ${SRC_DIR}/chrome/VERSION"
declare -A version_parts=()
while IFS='=' read -r key value; do
    [[ -n "$key" ]] && version_parts[$key]="$value"
done <"${SRC_DIR}/chrome/VERSION"
tree_version="${version_parts[MAJOR]:-}.${version_parts[MINOR]:-}.${version_parts[BUILD]:-}.${version_parts[PATCH]:-}"
pinned_version="$(<"${OVERLAY_DIR}/CHROMIUM_VERSION")"
[[ "$tree_version" == "$pinned_version" ]] ||
    die "tree is Chromium ${tree_version}; the overlay is pinned to ${pinned_version} (see browser/rebase.sh)"

# Concurrent overlay applications must not observe half-applied patches.
exec 9>"${SRC_DIR}/.openside-overlay.lock"
flock -n 9 || die 'another overlay application holds the lock'

desired=()
while IFS= read -r entry || [[ -n "$entry" ]]; do
    [[ -z "$entry" || "$entry" == \#* ]] && continue
    [[ "$entry" =~ ^[[:alnum:]][[:alnum:]._-]*\.patch$ && -f "${OVERLAY_DIR}/patches/${entry}" ]] ||
        die "invalid or missing series entry: ${entry}"
    desired+=("$entry")
done <"${OVERLAY_DIR}/patches/series"
[[ ${#desired[@]} -gt 0 ]] || die 'empty patch series'

patch_quiet() {
    patch --batch --forward --fuzz=0 -d "$SRC_DIR" -p1 "$@"
}

record_applied() {
    local temp
    temp="$(mktemp "${STATE}/applied.list.XXXXXX")"
    if [[ $# -gt 0 ]]; then
        printf '%s\n' "$@" >"$temp"
    fi
    mv -- "$temp" "${STATE}/applied.list"
}

# Trees prepared before per-patch state existed carry only a digest stamp for
# the first patch; adopt it only if that patch provably is applied.
if [[ -f "$LEGACY_STAMP" && ! -d "$STATE" ]]; then
    first="${desired[0]}"
    patch --batch --reverse --fuzz=0 -d "$SRC_DIR" -p1 --dry-run \
        -i "${OVERLAY_DIR}/patches/${first}" >/dev/null ||
        die 'legacy overlay stamp does not match the first patch; use a fresh pinned base tree'
    mkdir -p "${STATE}/applied"
    cp -- "${OVERLAY_DIR}/patches/${first}" "${STATE}/applied/${first}"
    record_applied "$first"
fi
mkdir -p "${STATE}/applied"
[[ -f "${STATE}/applied.list" ]] || record_applied

applied=()
while IFS= read -r entry; do
    [[ -n "$entry" ]] && applied+=("$entry")
done <"${STATE}/applied.list"

keep=0
while [[ $keep -lt ${#applied[@]} && $keep -lt ${#desired[@]} &&
    "${applied[$keep]}" == "${desired[$keep]}" ]] &&
    cmp -s -- "${STATE}/applied/${applied[$keep]}" "${OVERLAY_DIR}/patches/${desired[$keep]}"; do
    keep=$((keep + 1))
done

for ((i = ${#applied[@]} - 1; i >= keep; i--)); do
    name="${applied[$i]}"
    recorded="${STATE}/applied/${name}"
    patch --batch --reverse --fuzz=0 -d "$SRC_DIR" -p1 --dry-run -i "$recorded" >/dev/null ||
        die "cannot reverse ${name}: the tree diverged from the recorded patch; use a fresh pinned base tree"
    patch --batch --reverse --fuzz=0 -d "$SRC_DIR" -p1 -i "$recorded" >/dev/null
    rm -f -- "$recorded"
    record_applied "${applied[@]:0:$i}"
    printf 'Reverted %s\n' "$name"
done

cp -a -- "${OVERLAY_DIR}/src/." "${SRC_DIR}/"

current=("${desired[@]:0:$keep}")
for ((i = keep; i < ${#desired[@]}; i++)); do
    name="${desired[$i]}"
    if ! output="$(patch_quiet --dry-run -i "${OVERLAY_DIR}/patches/${name}" 2>&1)"; then
        printf '%s\n' "$output" >&2
        die "${name} does not apply cleanly ($((i + 1))/${#desired[@]}); stopped with ${#current[@]} patch(es) applied. Refresh the patch or run browser/rebase.sh."
    fi
    patch_quiet -i "${OVERLAY_DIR}/patches/${name}" >/dev/null
    cp -- "${OVERLAY_DIR}/patches/${name}" "${STATE}/applied/${name}"
    current+=("$name")
    record_applied "${current[@]}"
    printf 'Applied %s\n' "$name"
done

digest="$(
    cd -- "$OVERLAY_DIR"
    export LC_ALL=C
    shopt -s globstar nullglob
    inputs=(CHROMIUM_VERSION args.gn patches/series)
    for file in src/**/* patches/*.patch; do
        [[ -f "$file" ]] && inputs+=("$file")
    done
    sha256sum "${inputs[@]}" | sha256sum
)"
printf '%s\n' "${digest%% *}" >"$LEGACY_STAMP"
printf 'openside overlay applied: %d patch(es), digest %s\n' "${#desired[@]}" "${digest%% *}"
