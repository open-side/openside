import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter, Route, Routes, useParams } from "react-router";
import { describe, expect, it, vi } from "vitest";
import { ClientContext } from "../context";
import { fakeClient, json, type Call } from "../test/fakeDaemon";
import { NewChatPage } from "./NewChatPage";

function Opened() {
  return <p>Opened {useParams().id}</p>;
}

function setup() {
  const daemon = fakeClient((call: Call) => {
    if (call.path === "/v1/settings") return json(200, { default_model: "script", fallback_models: ["provider:big"], policy_mode: "guard" });
    if (call.method === "POST" && call.path === "/v1/sessions") return json(201, { id: "s9", ...(typeof call.body === "object" ? call.body : {}) });
    return json(202, { id: "m", status: "queued" });
  });
  render(
    <ClientContext.Provider value={daemon.client}>
      <MemoryRouter>
        <Routes>
          <Route index element={<NewChatPage />} />
          <Route path="sessions/:id" element={<Opened />} />
        </Routes>
      </MemoryRouter>
    </ClientContext.Provider>,
  );
  return daemon;
}

describe("NewChatPage", () => {
  it("creates the conversation from the first message with the picked model and opens it", async () => {
    const user = userEvent.setup();
    const { calls } = setup();
    expect(screen.getByRole("heading", { name: "What’s on your mind?" })).toBeInTheDocument();

    await user.click(await screen.findByRole("button", { name: /Default \(script\)/ }));
    const models = await screen.findByRole("radiogroup", { name: "Model" });
    expect(within(models).getAllByRole("radio").map((radio) => radio.textContent)).toEqual(["Default (script)", "script", "provider:big"]);
    await user.click(within(models).getByRole("radio", { name: "provider:big" }));
    await vi.waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    expect(screen.getByRole("button", { name: /provider:big/ })).toBeInTheDocument();

    await user.type(screen.getByRole("textbox", { name: "Message" }), "Which color?\nPick one.");
    await user.click(screen.getByRole("button", { name: "Send" }));
    expect(await screen.findByText("Opened s9")).toBeInTheDocument();
    const posts = calls.filter((c) => c.method === "POST" && !c.path.includes("/prewarm"));
    expect(posts[0]).toMatchObject({ path: "/v1/sessions", body: { model: "provider:big" } });
    expect(posts[0]?.body).not.toHaveProperty("title");
    expect(posts[0]?.body).not.toHaveProperty("browser_space");
    expect(posts[1]).toMatchObject({ path: "/v1/sessions/s9/messages", body: { text: "Which color?\nPick one.", mode: "queue" } });
  });

  it("starts an isolated-browser conversation only when the toggle is on", async () => {
    const user = userEvent.setup();
    const { calls } = setup();
    const toggle = screen.getByRole("button", { name: "Isolated browser" });
    expect(toggle).toHaveAttribute("aria-pressed", "false");
    await user.click(toggle);
    expect(toggle).toHaveAttribute("aria-pressed", "true");
    await user.type(screen.getByRole("textbox", { name: "Message" }), "Sign in fresh");
    await user.click(screen.getByRole("button", { name: "Send" }));
    expect(await screen.findByText("Opened s9")).toBeInTheDocument();
    expect(calls.find((c) => c.method === "POST" && !c.path.includes("/prewarm"))).toMatchObject({ path: "/v1/sessions", body: { browser_space: "isolated" } });
  });
});
