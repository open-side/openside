# Private headless Wayland desktop for tools/qa/headed-ux.sh sway.
# Arch matches the host, so host-built opensided and the patched Chromium run unchanged.
FROM docker.io/library/archlinux:latest
RUN pacman -Sy --noconfirm --needed sway grim wtype chromium python curl ttf-dejavu noto-fonts jq procps-ng \
        python-pywayland wlr-protocols pkgconf \
    && pacman -Scc --noconfirm
# Bindings for the wlr virtual pointer: headless sway has no pointer device otherwise.
RUN python3 -m pywayland.scanner -o /opt/wlproto \
        -i /usr/share/wayland/wayland.xml \
           /usr/share/wlr-protocols/unstable/wlr-virtual-pointer-unstable-v1.xml \
    && touch /opt/wlproto/__init__.py
