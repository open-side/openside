import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { installScrollReveal } from "./scrollReveal";

describe("installScrollReveal", () => {
  let list: HTMLElement;
  let uninstall: () => void;

  beforeEach(() => {
    vi.useFakeTimers();
    list = document.body.appendChild(document.createElement("div"));
    list.className = "thin-scroll";
    uninstall = installScrollReveal();
  });
  afterEach(() => {
    uninstall();
    list.remove();
    vi.useRealTimers();
  });

  it("lights the scrollbar for a scroll the reader caused, then lets it fade", () => {
    document.dispatchEvent(new Event("wheel"));
    list.dispatchEvent(new Event("scroll"));
    expect(list.dataset.scrolling).toBeDefined();
    vi.advanceTimersByTime(901);
    expect(list.dataset.scrolling).toBeUndefined();
  });

  it("ignores scrolls nobody asked for, such as the transcript pinning to a streaming turn", () => {
    list.dispatchEvent(new Event("scroll"));
    expect(list.dataset.scrolling).toBeUndefined();
  });

  it("ignores scrollers that did not opt in", () => {
    list.className = "";
    document.dispatchEvent(new Event("wheel"));
    list.dispatchEvent(new Event("scroll"));
    expect(list.dataset.scrolling).toBeUndefined();
  });
});
