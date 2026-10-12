import { useCallback, useEffect, useState } from "react";

const DISMISSED_KEY = "openside.install.dismissed";
/** Android may offer again after this long; the iOS hint is shown once. */
const REOFFER_MS = 14 * 24 * 60 * 60 * 1000;

/** Chrome's `beforeinstallprompt` event; not in lib.dom. */
interface InstallPromptEvent extends Event {
  prompt(): Promise<void>;
  userChoice: Promise<{ outcome: "accepted" | "dismissed" }>;
}

export type InstallOffer = { kind: "android"; install: () => Promise<void> } | { kind: "ios" };

/** iOS delivers Web Push and badges only to Home Screen apps; a Safari tab has neither. */
export function iosBrowserTab(): boolean {
  const ios = /iPad|iPhone|iPod/.test(navigator.userAgent) || (navigator.platform === "MacIntel" && navigator.maxTouchPoints > 1);
  return ios && !standalone();
}

export function standalone(): boolean {
  return (
    (typeof window.matchMedia === "function" &&
      ["standalone", "window-controls-overlay", "fullscreen", "minimal-ui"].some((mode) => window.matchMedia(`(display-mode: ${mode})`).matches)) ||
    (navigator as Navigator & { standalone?: boolean }).standalone === true
  );
}

// The event can fire before React mounts the banner, so it is captured when this module loads.
let deferred: InstallPromptEvent | null = null;
const listeners = new Set<() => void>();
const notify = () => listeners.forEach((listener) => listener());

if (typeof window !== "undefined") {
  window.addEventListener("beforeinstallprompt", (event) => {
    event.preventDefault();
    deferred = event as InstallPromptEvent;
    notify();
  });
  window.addEventListener("appinstalled", () => {
    deferred = null;
    notify();
  });
}

function dismissedRecently(now: number): boolean {
  const at = Number(localStorage.getItem(DISMISSED_KEY));
  return Number.isFinite(at) && at > 0 && now - at < REOFFER_MS;
}

/** Whether to show an install affordance now, and how it works on this platform. */
export function currentOffer(now = Date.now()): InstallOffer | null {
  if (standalone()) return null;
  if (deferred) {
    const event = deferred;
    return dismissedRecently(now)
      ? null
      : {
          kind: "android",
          install: async () => {
            deferred = null;
            await event.prompt();
            // The browser refuses a second prompt from the same event either way.
            await event.userChoice;
            notify();
          },
        };
  }
  // Once only: a person who closed the hint knows how Add to Home Screen works.
  return iosBrowserTab() && !localStorage.getItem(DISMISSED_KEY) ? { kind: "ios" } : null;
}

/** The install affordance for this device, or `null` when installed, dismissed or unsupported. */
export function useInstallOffer(): { offer: InstallOffer | null; dismiss: () => void } {
  const [, rerender] = useState(0);
  useEffect(() => {
    const update = () => rerender((n) => n + 1);
    listeners.add(update);
    return () => void listeners.delete(update);
  }, []);
  const dismiss = useCallback(() => {
    localStorage.setItem(DISMISSED_KEY, String(Date.now()));
    notify();
  }, []);
  // Desktop browsers have their own install control in the address bar; the banner is for phones and tablets.
  const touch = typeof window.matchMedia === "function" && window.matchMedia("(pointer: coarse)").matches;
  return { offer: touch ? currentOffer() : null, dismiss };
}
