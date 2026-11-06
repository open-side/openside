import { describe, expect, it } from "vitest";
import type { BrowserPrefs } from "../api/types";
import { addressTarget, DEFAULT_SEARCH_ENGINE, searchEngineOf } from "./AppHeader";

const prefs = (current: number | null, status: BrowserPrefs["status"] = "available"): BrowserPrefs =>
  ({
    status,
    prefs: status === "available" ? { search_engine: { current, options: [{ id: 1, name: "Google" }, { id: 92, name: "DuckDuckGo" }, { id: 7, name: "Some engine" }] } } : null,
  }) as BrowserPrefs;

describe("address field", () => {
  it("opens addresses as typed and searches anything else with the given engine", () => {
    expect(addressTarget("https://a.example/x")).toBe("https://a.example/x");
    expect(addressTarget("a.example/x")).toBe("https://a.example/x");
    expect(addressTarget("cats & dogs")).toBe("https://www.google.com/search?q=cats%20%26%20dogs");
    const ddg = searchEngineOf(prefs(92));
    expect(ddg).toEqual({ name: "DuckDuckGo", url: "https://duckduckgo.com/?q={q}" });
    expect(addressTarget("cats", ddg ?? DEFAULT_SEARCH_ENGINE)).toBe("https://duckduckgo.com/?q=cats");
    expect(addressTarget("   ")).toBeNull();
  });

  it("recognises localhost, literal, Unicode and intranet hosts without a scheme", () => {
    expect(addressTarget("localhost:5173/app")).toBe("http://localhost:5173/app");
    expect(addressTarget("localhost")).toBe("http://localhost/");
    expect(addressTarget("192.168.1.10/admin")).toBe("http://192.168.1.10/admin");
    expect(addressTarget("[::1]:8080/x")).toBe("http://[::1]:8080/x");
    expect(addressTarget("intranet:8080/wiki")).toBe("http://intranet:8080/wiki");
    expect(addressTarget("intranet/")).toBe("http://intranet/");
    expect(addressTarget("go/standup")).toBe("http://go/standup");
    expect(addressTarget("my_site.example")).toBe("https://my_site.example/");
    expect(addressTarget("bücher.example/a")).toBe("https://xn--bcher-kva.example/a");
    expect(addressTarget("https://bücher.example/a")).toBe("https://xn--bcher-kva.example/a");
  });

  it("searches text with spaces, a lone word and what is not a valid address", () => {
    expect(addressTarget("https://a.example/two words")).toBe("https://www.google.com/search?q=https%3A%2F%2Fa.example%2Ftwo%20words");
    expect(addressTarget("example.com is down")).toBe("https://www.google.com/search?q=example.com%20is%20down");
    expect(addressTarget("intranet")).toBe("https://www.google.com/search?q=intranet");
    expect(addressTarget("1.5")).toBe("https://www.google.com/search?q=1.5");
    expect(addressTarget("3/4")).toBe("https://www.google.com/search?q=3%2F4");
    expect(addressTarget("https://exa mple/")).toBe("https://www.google.com/search?q=https%3A%2F%2Fexa%20mple%2F");
  });

  it("never navigates to a non-http scheme: javascript:, data: and file: are searched as text", () => {
    expect(addressTarget("javascript:alert(1)")).toBe("https://www.google.com/search?q=javascript%3Aalert(1)");
    expect(addressTarget("JAVASCRIPT:alert(document.cookie)")).toBe("https://www.google.com/search?q=JAVASCRIPT%3Aalert(document.cookie)");
    expect(addressTarget("data:text/html,<b>x</b>")).toBe("https://www.google.com/search?q=data%3Atext%2Fhtml%2C%3Cb%3Ex%3C%2Fb%3E");
    expect(addressTarget("file:///etc/passwd")).toBe("https://www.google.com/search?q=file%3A%2F%2F%2Fetc%2Fpasswd");
    for (const input of ["chrome://settings", "ftp://a.example/f"]) {
      expect(addressTarget(input), input).toBe(`https://www.google.com/search?q=${encodeURIComponent(input)}`);
    }
  });

  it("fills a custom engine's template with the encoded query", () => {
    expect(addressTarget("ä & b?", { name: "Custom", url: "https://search.example/find?term={q}&src=os" })).toBe("https://search.example/find?term=%C3%A4%20%26%20b%3F&src=os");
  });

  it("finds the engine by Chromium's prepopulated id, whatever the browser's locale calls it", () => {
    const korean = { status: "available", prefs: { search_engine: { current: 67, options: [{ id: 1, name: "Google" }, { id: 67, name: "네이버" }, { id: 68, name: "Daum" }] } } } as BrowserPrefs;
    const naver = searchEngineOf(korean);
    expect(naver).toEqual({ name: "네이버", url: "https://search.naver.com/search.naver?query={q}" });
    expect(addressTarget("날씨 서울", naver ?? DEFAULT_SEARCH_ENGINE)).toBe("https://search.naver.com/search.naver?query=%EB%82%A0%EC%94%A8%20%EC%84%9C%EC%9A%B8");
    const japan = { status: "available", prefs: { search_engine: { current: 2, options: [{ id: 2, name: "Yahoo! JAPAN" }] } } } as BrowserPrefs;
    expect(searchEngineOf(japan)).toEqual({ name: "Yahoo! JAPAN", url: "https://search.yahoo.com/search?p={q}" });
    // A known id keeps working without its option row; an unknown id is not rescued by a familiar name.
    expect(searchEngineOf({ status: "available", prefs: { search_engine: { current: 3, options: [] } } } as unknown as BrowserPrefs)).toEqual({ name: "Microsoft Bing", url: "https://www.bing.com/search?q={q}" });
    expect(searchEngineOf({ status: "available", prefs: { search_engine: { current: 999, options: [{ id: 999, name: "Google" }] } } } as BrowserPrefs)).toBeNull();
  });

  it("falls back to Google when the browser's engine is unknown or unreadable", () => {
    expect(searchEngineOf(prefs(7))).toBeNull();
    expect(searchEngineOf(prefs(null))).toBeNull();
    expect(searchEngineOf(prefs(92, "not_running"))).toBeNull();
  });
});
