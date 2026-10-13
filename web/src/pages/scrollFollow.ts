/** Distance from the bottom (px) that still counts as "following" the live turn. */
export const FOLLOW_SLACK = 80;

export interface ScrollSample {
  top: number;
  height: number;
  client: number;
}

export const distanceFromEnd = (sample: ScrollSample): number => sample.height - sample.top - sample.client;

/**
 * Whether the transcript keeps pinned to its latest turn after a scroll event. Leaving the
 * bottom (even by a few pixels) stops following at once, so a streaming turn never drags the
 * reader back; returning within {@link FOLLOW_SLACK} resumes it. Moves down that are still far
 * from the end keep the current state, because a smooth jump to latest passes through them.
 * A shrinking scrollHeight clamps scrollTop without the reader moving, so it is not "up".
 */
export function nextFollowing(following: boolean, previous: ScrollSample, current: ScrollSample): boolean {
  const near = distanceFromEnd(current) < FOLLOW_SLACK;
  // 1px of tolerance: at fractional device scales the browser rounds a pinned scrollTop.
  const movedUp = current.top < previous.top - 1 && current.height >= previous.height;
  if (movedUp) return false;
  return near || following;
}
