#!/usr/bin/env bash
# Desktop-notification QA for the patched openside browser. See tools/qa/desktop_notify.py.
#
#   tools/qa/desktop-notify.sh sway <repro|smoke> [args...]
#       Private headless sway + session D-Bus + mako in a rootless, network-less podman container;
#       never touches a host desktop. mako is the notification server, so the notification the
#       browser sends is rendered (grim screenshot) and listed (`makoctl history`).
#
# Prerequisites: target/debug/opensided, web/dist, agent/.venv, the patched build
# (OPENSIDE_PATCHED_BROWSER or ../openside-browser-build) and, for smoke, a test build of the
# extension: OPENSIDE_EXTENSION_OUTDIR=$OPENSIDE_QA_OUT/extension OPENSIDE_EXTENSION_TEST_MATCHES=1 pnpm -C extension build
set -euo pipefail
ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd -P)"
desktop="${1:-sway}"
scenario="${2:-smoke}"
shift $(( $# >= 2 ? 2 : $# ))
out="${OPENSIDE_QA_OUT:-/tmp/openside-qa/round2/NotifyFix}"
mkdir -p "$out"
python="$ROOT/agent/.venv/bin/python"
patched="${OPENSIDE_PATCHED_BROWSER:-$ROOT/../openside-browser-build/portablelinux/build/src/out/Default/chrome}"
patched="$(realpath -- "$patched")"  # the container mounts the real directory; a ../ path would not resolve
[ "$desktop" = sway ] || { echo "desktop: sway" >&2; exit 2; }

image=localhost/openside-qa-notify:latest
podman image exists "$image" || podman build -t "$image" -f "$ROOT/tools/qa/desktop-notify.Containerfile" "$ROOT/tools/qa"
# The venv links to uv's managed interpreters through a version alias; mount them all.
uv_python="$(dirname -- "$(dirname -- "$(dirname -- "$(realpath -- "$python")")")")"
browser_dir="$(dirname -- "$(realpath -- "$patched")")"
exec podman run --rm --userns=keep-id --network=none --shm-size=1g \
    -v "$ROOT:$ROOT:ro" -v "$browser_dir:$browser_dir:ro" -v "$uv_python:$uv_python:ro" -v "$out:$out" \
    -e OPENSIDE_PATCHED_BROWSER="$patched" "$image" bash -c '
set -euo pipefail
# sway ships with file capabilities that a rootless user namespace cannot grant; a copy has none.
cp /usr/bin/sway /tmp/sway
export XDG_RUNTIME_DIR=/tmp/xdg; mkdir -m700 -p "$XDG_RUNTIME_DIR"
printf "output HEADLESS-1 resolution 1600x1000\n" >/tmp/sway.conf
WLR_BACKENDS=headless WLR_LIBINPUT_NO_DEVICES=1 WLR_RENDERER=pixman /tmp/sway -c /tmp/sway.conf >/tmp/sway.log 2>&1 &
for _ in $(seq 50); do ls "$XDG_RUNTIME_DIR"/sway-ipc.* >/dev/null 2>&1 && break; sleep 0.1; done
export WAYLAND_DISPLAY=wayland-1 SWAYSOCK="$(ls "$XDG_RUNTIME_DIR"/sway-ipc.*)"
eval "$(dbus-launch --sh-syntax)"
mako --default-timeout 0 --font "DejaVu Sans 12" >/tmp/mako.log 2>&1 &
sleep 1
status=0
"'"$python"'" "'"$ROOT"'/tools/qa/desktop_notify.py" '"$scenario"' --headed --no-sandbox --out "'"$out"'" '"$*"' || status=1
makoctl history >"'"$out"'/mako-history.txt" 2>&1 || true
cp /tmp/mako.log "'"$out"'/mako.log" 2>/dev/null || true
exit "$status"'
