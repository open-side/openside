# headed-ux image plus a Wayland notification daemon, so tools/qa/desktop-notify.sh can see the
# notifications the browser really sends over D-Bus (mako renders them in the private sway).
FROM localhost/openside-qa-uxnative:latest
USER root
RUN pacman -Sy --noconfirm --needed mako && pacman -Scc --noconfirm
