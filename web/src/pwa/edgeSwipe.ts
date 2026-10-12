import { useEffect } from "react";

/** Starts inside this band along the left edge; wider than a thumb-sized slop, narrower than a button. */
const EDGE_PX = 24;
const OPEN_DISTANCE_PX = 56;
/** The move must be mostly horizontal, or it is a scroll that happened to start near the edge. */
const MAX_SLOPE = 0.6;
const PHONE_QUERY = "(max-width: 820px)";

/** Opens the phone sidebar with a swipe from the left screen edge, like a native back/drawer gesture. */
export function useEdgeSwipe(onOpen: () => void): void {
  useEffect(() => {
    let start: { x: number; y: number } | null = null;
    const begin = (event: TouchEvent) => {
      start = null;
      const touch = event.touches[0];
      if (!touch || event.touches.length !== 1 || touch.clientX > EDGE_PX) return;
      if (!window.matchMedia?.(PHONE_QUERY).matches) return;
      // A dialog, sheet or the drawer itself owns touches while it is open.
      if (document.querySelector('[role="dialog"]')) return;
      start = { x: touch.clientX, y: touch.clientY };
    };
    const track = (event: TouchEvent) => {
      const touch = event.touches[0];
      if (!start || !touch) return;
      const dx = touch.clientX - start.x;
      const dy = Math.abs(touch.clientY - start.y);
      if (dy > 12 && dy > dx * MAX_SLOPE) start = null;
      else if (dx >= OPEN_DISTANCE_PX) {
        start = null;
        onOpen();
      }
    };
    const end = () => {
      start = null;
    };
    document.addEventListener("touchstart", begin, { passive: true });
    document.addEventListener("touchmove", track, { passive: true });
    document.addEventListener("touchend", end, { passive: true });
    document.addEventListener("touchcancel", end, { passive: true });
    return () => {
      document.removeEventListener("touchstart", begin);
      document.removeEventListener("touchmove", track);
      document.removeEventListener("touchend", end);
      document.removeEventListener("touchcancel", end);
    };
  }, [onOpen]);
}
