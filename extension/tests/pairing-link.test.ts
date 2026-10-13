import { describe, expect, it } from "vitest";
import { parsePairingLink } from "../src/storage";

describe("pairing link", () => {
  it("reads the daemon origin and code from a direct link", () => {
    expect(parsePairingLink(" http://127.0.0.1:47820/pair#K7QM2XPD ")).toEqual({ url: "http://127.0.0.1:47820", code: "K7QM2XPD" });
    expect(parsePairingLink("https://desk.tail1234.ts.net/pair#AB%20CD")).toEqual({ url: "https://desk.tail1234.ts.net", code: "AB CD" });
  });

  it("refuses relay links, links without a code and other pages", () => {
    expect(() => parsePairingLink("https://relay.example/pair#c=X&t=relay&d=1&k=2")).toThrow(/direct openside pairing link/);
    expect(() => parsePairingLink("https://desk.example/pair")).toThrow(/direct openside pairing link/);
    expect(() => parsePairingLink("https://desk.example/sessions#X")).toThrow(/direct openside pairing link/);
  });

  it("applies the daemon URL rules: plain HTTP only on loopback", () => {
    expect(() => parsePairingLink("http://desk.example/pair#X")).toThrow(/HTTPS/);
    expect(() => parsePairingLink("not a link")).toThrow(TypeError);
  });
});
