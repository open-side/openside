#!/usr/bin/env bash
# Runs `ninja` for the Chromium tree under the shared-machine resource guard:
#   systemd user scope (MemoryMax=24G, CPUWeight=20) + nice 19 + ionice idle,
#   ninja -j <= 14, a memory watchdog that SIGSTOPs the scope while
#   MemAvailable < 8 GiB and SIGCONTs it above 12 GiB, and a disk watchdog that
#   stops the build when the build workspace exceeds 150 GiB.
#
# Usage: guarded-build.sh [NINJA_TARGET...]     (default target: chrome)
# Environment:
#   OPENSIDE_BROWSER_BUILD_DIR  build workspace (required unless OPENSIDE_CHROMIUM_SRC is set)
#   OPENSIDE_CHROMIUM_SRC       source tree; default $OPENSIDE_BROWSER_BUILD_DIR/portablelinux/build/src
#   OPENSIDE_NINJA_JOBS         1..14, default 8
#   OPENSIDE_BUILD_LOG          append guard and ninja output to this file instead of stdout
set -euo pipefail

die() {
    printf 'guarded-build.sh: %s\n' "$*" >&2
    exit 1
}

# Every resource-affecting ninja option (-j, -l, -C, -f, ...) stays under this
# wrapper's control, so positional arguments may only name build targets.
for target in "$@"; do
    [[ "$target" =~ ^[[:alnum:]_./:+][[:alnum:]_./:+-]*$ ]] ||
        die "invalid ninja target '${target}': only target names are accepted"
done

if [[ -n "${OPENSIDE_CHROMIUM_SRC:-}" ]]; then
    SRC_DIR="$OPENSIDE_CHROMIUM_SRC"
else
    : "${OPENSIDE_BROWSER_BUILD_DIR:?set OPENSIDE_BROWSER_BUILD_DIR to the Chromium build workspace}"
    SRC_DIR="${OPENSIDE_BROWSER_BUILD_DIR}/portablelinux/build/src"
fi
SRC_DIR="$(cd -- "$SRC_DIR" && pwd -P)"
WORKSPACE="${OPENSIDE_BROWSER_BUILD_DIR:-$(dirname -- "$SRC_DIR")}"
JOBS="${OPENSIDE_NINJA_JOBS:-8}"
[[ "$JOBS" =~ ^([1-9]|1[0-4])$ ]] || die 'OPENSIDE_NINJA_JOBS must be between 1 and 14'
targets=("$@")
[[ ${#targets[@]} -gt 0 ]] || targets=(chrome)
for tool in ninja systemd-run systemctl nice ionice du; do
    command -v "$tool" >/dev/null || die "missing tool: $tool"
done
[[ -f "${SRC_DIR}/out/Default/build.ninja" ]] ||
    die 'generate the pinned base GN build (out/Default) before building'

if [[ -n "${OPENSIDE_BUILD_LOG:-}" ]]; then
    exec >>"$OPENSIDE_BUILD_LOG" 2>&1
fi

DISK_LIMIT_KIB=$((150 * 1024 * 1024))
workspace_kib() {
    local usage
    usage="$(du -sk -- "$WORKSPACE")"
    printf '%s\n' "${usage%%[[:space:]]*}"
}
[[ "$(workspace_kib)" -le $DISK_LIMIT_KIB ]] || die 'build workspace exceeds the 150 GiB limit'

# Non-login shells may lack the user bus environment systemd-run --user needs.
export XDG_RUNTIME_DIR="${XDG_RUNTIME_DIR:-/run/user/$(id -u)}"
export DBUS_SESSION_BUS_ADDRESS="${DBUS_SESSION_BUS_ADDRESS:-unix:path=${XDG_RUNTIME_DIR}/bus}"
UNIT="openside-browser-build-$$"

available_memory_kib() {
    local key value _
    while read -r key value _; do
        if [[ "$key" == 'MemAvailable:' ]]; then
            printf '%s\n' "$value"
            return
        fi
    done </proc/meminfo
    return 1
}

# Do not start a memory-heavy build while another session already needs the RAM.
while [[ "$(available_memory_kib)" -lt $((8 * 1024 * 1024)) ]]; do
    printf '=== [guard] %s waiting for at least 8 GiB MemAvailable\n' "$(date -Is)"
    sleep 15
done

watchdog() {
    local paused=0 available disk_ticks=0
    while sleep 15; do
        available="$(available_memory_kib)"
        if [[ "$paused" -eq 0 && "$available" -lt $((8 * 1024 * 1024)) ]]; then
            printf '=== [watchdog] %s MemAvailable=%skB < 8GiB -> SIGSTOP %s\n' "$(date -Is)" "$available" "$UNIT"
            systemctl --user kill --signal=SIGSTOP "${UNIT}.scope" || true
            paused=1
        elif [[ "$paused" -eq 1 && "$available" -gt $((12 * 1024 * 1024)) ]]; then
            printf '=== [watchdog] %s MemAvailable=%skB > 12GiB -> SIGCONT %s\n' "$(date -Is)" "$available" "$UNIT"
            systemctl --user kill --signal=SIGCONT "${UNIT}.scope" || true
            paused=0
        fi
        disk_ticks=$((disk_ticks + 1))
        if [[ "$disk_ticks" -eq 8 ]]; then
            disk_ticks=0
            if [[ "$(workspace_kib)" -gt $DISK_LIMIT_KIB ]]; then
                printf '=== [watchdog] %s workspace above 150 GiB -> stopping build\n' "$(date -Is)"
                systemctl --user kill --signal=SIGKILL "${UNIT}.scope" || true
                return 1
            fi
        fi
    done
}

watchdog &
WATCHDOG_PID=$!
cleanup() {
    kill "$WATCHDOG_PID" 2>/dev/null || true
    wait "$WATCHDOG_PID" 2>/dev/null || true
    # A trapped signal must not leave a paused scope or an orphaned compiler.
    systemctl --user kill --signal=SIGCONT "${UNIT}.scope" 2>/dev/null || true
    systemctl --user kill --signal=SIGKILL "${UNIT}.scope" 2>/dev/null || true
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM

started=$SECONDS
printf '=== [guard] %s unit=%s jobs=%s targets: %s\n' "$(date -Is)" "$UNIT" "$JOBS" "${targets[*]}"
set +e
systemd-run --user --scope --quiet --unit="$UNIT" -p MemoryMax=24G -p CPUWeight=20 \
    nice -n 19 ionice -c3 ninja -C "${SRC_DIR}/out/Default" -j "$JOBS" "${targets[@]}"
rc=$?
set -e
printf '=== [guard] %s exit=%s elapsed=%ss\n' "$(date -Is)" "$rc" "$((SECONDS - started))"
exit "$rc"
