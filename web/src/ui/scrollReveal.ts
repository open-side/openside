const HOLD_MS = 900;
const INPUT_WINDOW_MS = 1000;

const timers = new WeakMap<HTMLElement, number>();

/**
 * Scrollbars on `.thin-scroll` elements stay invisible until the reader scrolls (`data-scrolling`
 * for a moment) or hovers, like an overlay scrollbar. Only scrolls that follow a wheel, touch,
 * key or pointer input count: the transcript pinning itself to a streaming turn must not
 * keep the thumb lit. Scroll events do not bubble, so one capturing listener serves them all.
 */
export function installScrollReveal(root: Document = document): () => void {
  let lastInput = Number.NEGATIVE_INFINITY;
  const markInput = () => {
    lastInput = performance.now();
  };
  const onScroll = (event: Event) => {
    const target = event.target;
    if (!(target instanceof HTMLElement) || !target.classList.contains("thin-scroll")) return;
    if (performance.now() - lastInput > INPUT_WINDOW_MS) return;
    target.dataset.scrolling = "";
    window.clearTimeout(timers.get(target));
    timers.set(
      target,
      window.setTimeout(() => {
        delete target.dataset.scrolling;
      }, HOLD_MS),
    );
  };
  const inputs = ["wheel", "touchmove", "keydown", "pointerdown"];
  const options = { capture: true, passive: true } as const;
  for (const type of inputs) root.addEventListener(type, markInput, options);
  root.addEventListener("scroll", onScroll, options);
  return () => {
    for (const type of inputs) root.removeEventListener(type, markInput, options);
    root.removeEventListener("scroll", onScroll, options);
  };
}
