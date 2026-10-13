import { afterEach, describe, expect, it } from "vitest";
import { clearToken, loadLocalSecret, loadToken, saveToken, thisDevice } from "./auth";
import { applyLocalAccess, consumeLocalAccessFragment, parseLocalAccess } from "./localAccess";

afterEach(() => {
  localStorage.clear();
  sessionStorage.clear();
  window.history.replaceState(null, "", "/");
});

describe("parseLocalAccess", () => {
  it("reads the launcher fragment, a full launch link and the CLI's JSON", () => {
    const expected = { secret: "s3", token: "tok", device: "dev1" };
    expect(parseLocalAccess("#local_secret=s3&local_token=tok&local_device=dev1")).toEqual(expected);
    expect(parseLocalAccess("http://127.0.0.1:47820/#local_secret=s3&local_token=tok&local_device=dev1")).toEqual(expected);
    expect(parseLocalAccess('{"secret":"s3","url":null,"device_id":"dev1","token":"tok","scopes":["admin"],"kind":"person_device"}')).toEqual(expected);
  });

  it("takes a bare string as the secret alone and refuses anything else", () => {
    expect(parseLocalAccess("  s3cret ")).toEqual({ secret: "s3cret", token: null, device: null });
    expect(parseLocalAccess("")).toBeNull();
    expect(parseLocalAccess("two words")).toBeNull();
    expect(parseLocalAccess("#other=1")).toBeNull();
    expect(parseLocalAccess("{not json")).toBeNull();
  });
});

describe("consumeLocalAccessFragment", () => {
  it("keeps the secret for the tab only, stores the admin token and removes the fragment at once", () => {
    window.history.replaceState(null, "", "/remote?x=1#local_secret=s3&local_token=tok&local_device=dev1");
    consumeLocalAccessFragment();
    expect(window.location.hash).toBe("");
    expect(window.location.pathname + window.location.search).toBe("/remote?x=1");
    expect(loadLocalSecret()).toBe("s3");
    expect(localStorage.getItem("openside.local-secret")).toBeNull();
    expect(loadToken()).toBe("tok");
    expect(thisDevice()).toBe("dev1");
  });

  it("leaves unrelated fragments alone", () => {
    window.history.replaceState(null, "", "/pair#CODE123");
    consumeLocalAccessFragment();
    expect(window.location.hash).toBe("#CODE123");
    expect(loadLocalSecret()).toBeNull();
  });

  it("replaces a leftover relay route when a local token arrives", () => {
    localStorage.setItem("openside.transport", JSON.stringify({ kind: "relay", relay: "https://r", daemon_id: "i", daemon_key: "k" }));
    expect(applyLocalAccess({ secret: "s", token: "t", device: null })).toBe(true);
    expect(localStorage.getItem("openside.transport")).toBeNull();
  });

  it("stores only the secret when no token was given", () => {
    localStorage.setItem("openside.token", "existing");
    expect(applyLocalAccess({ secret: "s", token: null, device: null })).toBe(false);
    expect(loadToken()).toBe("existing");
    expect(loadLocalSecret()).toBe("s");
  });
});

describe("local authority cleanup", () => {
  it("clears both storage classes on sign-out without deleting unrelated preferences", () => {
    applyLocalAccess({ secret: "boot", token: "owner", device: "d1" });
    localStorage.setItem("openside.transport", "previous-route");
    localStorage.setItem("appearance", "dark");
    sessionStorage.setItem("draft", "keep");
    clearToken();
    expect(loadToken()).toBeNull();
    expect(thisDevice()).toBeNull();
    expect(localStorage.getItem("openside.scopes")).toBeNull();
    expect(localStorage.getItem("openside.transport")).toBeNull();
    expect(loadLocalSecret()).toBeNull();
    expect(localStorage.getItem("appearance")).toBe("dark");
    expect(sessionStorage.getItem("draft")).toBe("keep");
  });

  it("drops the prior boot proof, device and route when changing identity", () => {
    applyLocalAccess({ secret: "boot", token: "owner", device: "d1" });
    localStorage.setItem("openside.transport", "old-route");
    saveToken("different-person");
    expect(loadToken()).toBe("different-person");
    expect(loadLocalSecret()).toBeNull();
    expect(thisDevice()).toBeNull();
    expect(localStorage.getItem("openside.transport")).toBeNull();
  });

  it("installs the replacement bootstrap proof after rotating the token", () => {
    applyLocalAccess({ secret: "old-boot", token: "old-owner", device: "d1" });
    applyLocalAccess({ secret: "new-boot", token: "new-owner", device: "d2" });
    expect(loadToken()).toBe("new-owner");
    expect(loadLocalSecret()).toBe("new-boot");
    expect(thisDevice()).toBe("d2");
  });
});
