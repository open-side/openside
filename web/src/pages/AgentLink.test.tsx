import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router";
import { describe, expect, it } from "vitest";
import { ClientContext } from "../context";
import { offer } from "../remote/testDaemon";
import { fakeClient, json, type Call } from "../test/fakeDaemon";
import { AgentLink } from "./AgentLink";

function renderLink(handler: (call: Call) => Response) {
  const fake = fakeClient(handler);
  render(
    <MemoryRouter>
      <ClientContext.Provider value={fake.client}>
        <AgentLink />
      </ClientContext.Provider>
    </MemoryRouter>,
  );
  return fake;
}

describe("AgentLink", () => {
  it("creates an agent link from the screen, with no command needed to issue it", async () => {
    const user = userEvent.setup();
    const fake = renderLink((call) =>
      call.method === "POST" && call.path === "/v1/pairing"
        ? json(200, offer({ kind: "agent_client", scopes: ["read", "steer", "browse", "approve"], url: "http://127.0.0.1:47820/pair#AGENT123" }))
        : json(404, { code: "not_found", message: "nope" }),
    );
    expect(screen.queryByText(/openside pair/)).toBeNull();
    await user.clear(screen.getByLabelText("Agent name"));
    await user.type(screen.getByLabelText("Agent name"), "CI box");
    await user.click(screen.getByRole("button", { name: "Create a link" }));
    expect(await screen.findByLabelText("Agent link")).toHaveValue("http://127.0.0.1:47820/pair#AGENT123");
    expect(fake.calls.find((call) => call.path === "/v1/pairing")?.body).toEqual({ name: "CI box", scopes: ["read", "steer", "browse", "approve"], kind: "agent_client", via: null });
    expect(screen.getByRole("status")).toHaveTextContent("expires in");
    expect(screen.getByText("openside login").closest("p")).toHaveTextContent("On the agent’s computer");
  });

  it("says issuing a link needs the computer itself when the daemon asks for local access", async () => {
    const user = userEvent.setup();
    renderLink(() => json(403, { code: "local_access_required", message: "local" }));
    await user.click(screen.getByRole("button", { name: "Create a link" }));
    expect(await screen.findByText(/use openside on the computer that runs it/)).toBeInTheDocument();
  });
});
