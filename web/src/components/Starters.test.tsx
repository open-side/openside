import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { ClientContext } from "../context";
import { fakeClient, json } from "../test/fakeDaemon";
import { siteHost, Starters } from "./Starters";

function mount(vault: unknown, status = 200) {
  const onPick = vi.fn();
  const daemon = fakeClient(() => json(status, vault));
  render(
    <ClientContext.Provider value={daemon.client}>
      <Starters onPick={onPick} />
    </ClientContext.Provider>,
  );
  return onPick;
}

describe("new-chat starters", () => {
  it("offers three ways to start and only fills the composer", async () => {
    const user = userEvent.setup();
    const onPick = mount({ available: false, default_policy: "while_unlocked", items: [] });
    expect(screen.getAllByRole("button")).toHaveLength(3);
    await user.click(screen.getByRole("button", { name: "Compare two pages" }));
    expect(onPick).toHaveBeenCalledWith(expect.stringMatching(/^Compare these two pages/));
    expect(screen.queryByRole("group", { name: "Your saved sites" })).toBeNull();
  });

  it("names the sites the vault already knows, from the real vault list", async () => {
    const user = userEvent.setup();
    const onPick = mount({
      available: true,
      default_policy: "while_unlocked",
      items: [
        { id: "shop", origins: ["https://*.shop.example", "https://www.shop.example"], has_totp: false, passkey_rp_ids: [], effective_policy: "while_unlocked" },
        { id: "naver", origins: ["https://nid.naver.com:443"], has_totp: false, passkey_rp_ids: [], effective_policy: "while_unlocked" },
      ],
    });
    const sites = await screen.findByRole("group", { name: "Your saved sites" });
    expect(sites.querySelectorAll("button")).toHaveLength(2);
    await user.click(screen.getByRole("button", { name: "Check shop.example for me" }));
    expect(onPick).toHaveBeenLastCalledWith("Sign in to shop.example and tell me what’s new.");
    await user.click(screen.getByRole("button", { name: "nid.naver.com" }));
    expect(onPick).toHaveBeenLastCalledWith("Sign in to nid.naver.com and tell me what’s new.");
  });

  it("shows no site row when the vault cannot be read", async () => {
    mount({ code: "vault_unavailable", message: "locked" }, 503);
    await vi.waitFor(() => expect(screen.getAllByRole("button")).toHaveLength(3));
    expect(screen.queryByRole("group", { name: "Your saved sites" })).toBeNull();
  });

  it("reduces an origin to the host a person would type", () => {
    expect(siteHost("https://*.shop.example:8443")).toBe("shop.example");
    expect(siteHost("not a url")).toBeNull();
  });
});
