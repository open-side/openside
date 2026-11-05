# Private headless Wayland desktop for tools/qa/headed-ux.sh sway.
# Arch matches the host, so host-built opensided, the patched Chromium and the host's
# agent/node_modules (glibc pi-natives addon) run unchanged.
FROM docker.io/library/archlinux:latest
RUN pacman -Sy --noconfirm --needed sway grim wtype chromium python python-websockets curl unzip ttf-dejavu noto-fonts \
        jq procps-ng python-pywayland wlr-protocols pkgconf \
    && pacman -Scc --noconfirm
# The sidecar runtime: the repository's minimum Bun, pinned so QA matches the gate. The checkout is
# mounted read-only, so its agent/node_modules must already be installed on the host.
RUN curl -fsSL https://bun.sh/install | BUN_INSTALL=/usr/local bash -s "bun-v1.4.4" \
    && bun --version
# Bindings for the wlr virtual pointer: headless sway has no pointer device otherwise.
RUN python3 -m pywayland.scanner -o /opt/wlproto \
        -i /usr/share/wayland/wayland.xml \
           /usr/share/wlr-protocols/unstable/wlr-virtual-pointer-unstable-v1.xml \
    && touch /opt/wlproto/__init__.py
