import { describe, expect, it } from "vitest";
import { distanceFromEnd, FOLLOW_SLACK, nextFollowing, type ScrollSample } from "./scrollFollow";

const sample = (top: number, height = 5000, client = 800): ScrollSample => ({ top, height, client });
const bottom = sample(4200);

describe("nextFollowing", () => {
  it("keeps following while the reader stays at the bottom, including as the turn grows", () => {
    expect(nextFollowing(true, bottom, bottom)).toBe(true);
    expect(nextFollowing(true, bottom, sample(4400, 5200))).toBe(true);
  });

  it("stops at the first upward move, even inside the follow slack", () => {
    expect(distanceFromEnd(sample(4170))).toBeLessThan(FOLLOW_SLACK);
    expect(nextFollowing(true, bottom, sample(4170))).toBe(false);
  });

  it("ignores sub-pixel rounding of a pinned scrollTop", () => {
    expect(nextFollowing(true, sample(4200.5), sample(4200))).toBe(true);
  });

  it("does not treat scrollTop clamped by shrinking content as the reader leaving", () => {
    expect(nextFollowing(true, bottom, sample(4100, 4900))).toBe(true);
  });

  it("stays detached while far from the bottom, even when moving down", () => {
    expect(nextFollowing(false, sample(1000), sample(1400))).toBe(false);
  });

  it("resumes within the follow slack of the bottom", () => {
    expect(nextFollowing(false, sample(4000), sample(4200 - FOLLOW_SLACK + 1))).toBe(true);
    expect(nextFollowing(false, sample(4000), sample(4200 - FOLLOW_SLACK))).toBe(false);
  });

  it("keeps following through a smooth jump to the latest turn", () => {
    expect(nextFollowing(true, sample(1000), sample(1900))).toBe(true);
  });
});
