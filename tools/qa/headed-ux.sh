#!/usr/bin/env bash
# Headed browser UX QA. See tools/qa/headed_ux.py for what is checked.
#
#   tools/qa/headed-ux.sh sway [patched|stock|both] [args...]
#       Private headless sway in a rootless, network-less podman container; never touches a host desktop.
#
# Prerequisites: target/debug/opensided, web/dist, extension/dist, agent/.venv,
# and for patched runs the patched build (OPENSIDE_PATCHED_BROWSER or ../openside-browser-build).
set -euo pipefail
ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd -P)"
desktop="${1:-sway}"
which="${2:-both}"
shift $(( $# >= 2 ? 2 : $# ))
case "$which" in both) browsers=(patched stock) ;; patched|stock) browsers=("$which") ;; *) echo "browser: patched|stock|both" >&2; exit 2 ;; esac
out="${OPENSIDE_QA_OUT:-/tmp/openside-qa/uxnative}"
mkdir -p "$out"
python="$ROOT/agent/.venv/bin/python"
patched="${OPENSIDE_PATCHED_BROWSER:-$ROOT/../openside-browser-build/portablelinux/build/src/out/Default/chrome}"

case "$desktop" in
sway)
    image=localhost/openside-qa-uxnative:latest
    podman image exists "$image" || podman build -t "$image" -f "$ROOT/tools/qa/headed-ux.Containerfile" "$ROOT/tools/qa"
    # The venv links to uv's managed interpreters through a version alias; mount them all.
    uv_python="$(dirname -- "$(dirname -- "$(dirname -- "$(realpath -- "$python")")")")"
    browser_mount=()
    if [ "$which" != stock ]; then
        browser_dir="$(dirname -- "$(realpath -- "$patched")")"
        browser_mount=(-v "$browser_dir:$browser_dir:ro")
    fi
    # Rootless and --network=none: daemon and browser talk over the container's loopback only.
    exec podman run --rm --userns=keep-id --network=none --shm-size=1g \
        -v "$ROOT:$ROOT:ro" "${browser_mount[@]}" -v "$uv_python:$uv_python:ro" -v "$out:$out" \
        -e OPENSIDE_PATCHED_BROWSER="$patched" "$image" bash -c '
set -euo pipefail
# sway ships with file capabilities that a rootless user namespace cannot grant; a copy has none.
cp /usr/bin/sway /tmp/sway
export XDG_RUNTIME_DIR=/tmp/xdg; mkdir -m700 -p "$XDG_RUNTIME_DIR"
printf "output HEADLESS-1 resolution 1600x1000\n" >/tmp/sway.conf
WLR_BACKENDS=headless WLR_LIBINPUT_NO_DEVICES=1 WLR_RENDERER=pixman /tmp/sway -c /tmp/sway.conf >/tmp/sway.log 2>&1 &
for _ in $(seq 50); do ls "$XDG_RUNTIME_DIR"/sway-ipc.* >/dev/null 2>&1 && break; sleep 0.1; done
export WAYLAND_DISPLAY=wayland-1 SWAYSOCK="$(ls "$XDG_RUNTIME_DIR"/sway-ipc.*)"
status=0
for browser in '"${browsers[*]}"'; do
    "'"$python"'" "'"$ROOT"'/tools/qa/headed_ux.py" --no-sandbox --browser "$browser" --out "'"$out"'" '"$*"' || status=1
done
exit "$status"'
    ;;
*) echo "desktop: sway" >&2; exit 2 ;;
esac
