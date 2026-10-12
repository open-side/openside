import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it } from "vitest";
import { ClientContext } from "../context";
import { fakeClient, json, type Call } from "../test/fakeDaemon";
import { Composer, MAX_ATTACHMENT_BYTES, MAX_ATTACHMENTS, MAX_TEXT_BYTES, mentionedSkills } from "./Composer";

const SKILLS = [
  { name: "research", description: "Deep web research", source: "user", enabled: true },
  { name: "review-pr", description: "Review a pull request", source: "project", enabled: true },
];

/** A request the person caused; focus also prewarms the agent, which tests ignore. */
const sent = (c: Call) => c.method === "POST" && !c.path.includes("/prewarm");

function setup() {
  const daemon = fakeClient((call: Call) => {
    const url = new URL(call.path, "http://daemon.test");
    if (url.pathname === "/v1/skills") {
      const q = url.searchParams.get("q") ?? "";
      return json(200, { skills: SKILLS.filter((s) => s.name.startsWith(q)), problems: [] });
    }
    return json(202, { id: "m", status: "queued" });
  });
  render(
    <ClientContext.Provider value={daemon.client}>
      <Composer sessionId="s1" />
    </ClientContext.Provider>,
  );
  return daemon;
}

describe("Composer skill mentions", () => {
  it("offers skills after `/`, inserts the chosen one as a chip and sends it in the text", async () => {
    const user = userEvent.setup();
    const { calls } = setup();
    const input = screen.getByRole("textbox", { name: "Message" });
    await user.type(input, "Please /re");
    const list = await screen.findByRole("listbox", { name: "Skills" });
    expect(within(list).getAllByRole("option").map((o) => o.textContent)).toEqual([
      "/research Deep web research",
      "/review-pr Review a pull request",
    ]);
    expect(calls.find((c) => c.method === "GET")?.path).toBe("/v1/skills?q=re&session=s1&enabled=true");
    await user.keyboard("{ArrowDown}{Enter}");
    expect(input).toHaveValue("Please /review-pr ");
    expect(screen.queryByRole("listbox")).toBeNull();
    const chips = screen.getByRole("list", { name: "Message context" });
    expect(chips).toHaveTextContent("/review-pr");

    await act(() => new Promise<void>((done) => requestAnimationFrame(() => done())));
    await user.type(input, "now", { skipClick: true });
    await user.click(screen.getByRole("button", { name: "Send" }));
    const submit = calls.find(sent);
    expect(submit).toMatchObject({ path: "/v1/sessions/s1/messages", body: { text: "Please /review-pr now", mode: "queue" } });
    expect(submit?.body).not.toHaveProperty("attachments");
  });

  it("removes a mention when its chip is dismissed", async () => {
    const user = userEvent.setup();
    setup();
    const input = screen.getByRole("textbox", { name: "Message" });
    await user.type(input, "/res");
    await user.click(await screen.findByRole("option", { name: /research/ }));
    // Choosing restores the caret after the inserted name on the next frame.
    await act(() => new Promise<void>((done) => requestAnimationFrame(() => done())));
    expect(input).toHaveValue("/research ");
    await user.type(input, "the topic", { skipClick: true });
    await user.click(screen.getByRole("button", { name: "Remove skill research" }));
    expect(input).toHaveValue("the topic");
    expect(screen.queryByRole("list", { name: "Message context" })).toBeNull();
  });

  it("dismisses mentions delimited by punctuation, not only by spaces", async () => {
    const user = userEvent.setup();
    setup();
    const input = screen.getByRole("textbox", { name: "Message" });
    await user.type(input, "/re");
    await screen.findByRole("option", { name: /review-pr/ });
    fireEvent.change(input, { target: { value: "Check (/review-pr). Then /research, please" } });
    expect(screen.getByRole("list", { name: "Message context" })).toHaveTextContent("/review-pr");
    await user.click(screen.getByRole("button", { name: "Remove skill review-pr" }));
    await user.click(screen.getByRole("button", { name: "Remove skill research" }));
    expect(input).toHaveValue("Check (). Then , please");
    expect(screen.queryByRole("list", { name: "Message context" })).toBeNull();
  });

  it("recognizes mentions with the sidecar's word rules", () => {
    const known = new Set(["research", "review-pr"]);
    expect(mentionedSkills("/research, then (/review-pr). /usr/bin /research/x /research.md /unknown /research", known)).toEqual([
      "research",
      "review-pr",
    ]);
  });
});

describe("Composer prewarm", () => {
  it("warms the session's agent once when the message box takes focus", async () => {
    const user = userEvent.setup();
    const { calls } = setup();
    const input = screen.getByRole("textbox", { name: "Message" });
    await user.click(input);
    input.blur();
    await user.click(input);
    await waitFor(() => expect(calls.filter((c) => c.path === "/v1/sessions/s1/prewarm")).toHaveLength(1));
    expect(calls.find((c) => c.path === "/v1/sessions/s1/prewarm")?.method).toBe("POST");
  });
});

describe("Composer attachments", () => {
  it("uploads picked files as base64 with the message and clears them", async () => {
    const user = userEvent.setup();
    const { calls } = setup();
    const png = new File([new Uint8Array([0x89, 0x50, 0x4e, 0x47])], "shot.png", { type: "image/png" });
    await user.upload(screen.getByLabelText("Attach files"), png);
    const chips = screen.getByRole("list", { name: "Message context" });
    expect(chips).toHaveTextContent("shot.png");
    expect(chips.querySelector("img.attachment-thumb")).toHaveAttribute("src", expect.stringMatching(/^blob:/));
    // An attachment alone is a valid message.
    await user.click(screen.getByRole("button", { name: "Send" }));
    await waitFor(() => expect(calls.some(sent)).toBe(true));
    expect(calls.find(sent)?.body).toMatchObject({
      text: "",
      attachments: [{ name: "shot.png", data_base64: "iVBORw==" }],
    });
    await waitFor(() => expect(screen.queryByRole("list", { name: "Message context" })).toBeNull());
  });

  it("drops a picked file before sending", async () => {
    const user = userEvent.setup();
    const { calls } = setup();
    await user.upload(screen.getByLabelText("Attach files"), new File(["notes"], "notes.txt", { type: "text/plain" }));
    await user.click(screen.getByRole("button", { name: "Remove attachment notes.txt" }));
    await user.type(screen.getByRole("textbox", { name: "Message" }), "hi");
    await user.click(screen.getByRole("button", { name: "Send" }));
    await waitFor(() => expect(calls.find(sent)?.body).toMatchObject({ text: "hi" }));
    expect(calls.find(sent)?.body).not.toHaveProperty("attachments");
  });

  const bytes = (name: string, size: number) => new File([new Uint8Array(size)], name, { type: "application/octet-stream" });
  const chipsOf = () => screen.queryAllByRole("button", { name: /^Remove attachment / }).length;

  it("accepts exactly the attachment count across batches and refuses one more", async () => {
    const user = userEvent.setup();
    setup();
    const picker = screen.getByLabelText("Attach files");
    await user.upload(picker, [bytes("a", 1), bytes("b", 1)]);
    await user.upload(picker, [bytes("c", 1), bytes("d", 1)]);
    expect(chipsOf()).toBe(MAX_ATTACHMENTS);
    expect(screen.queryByRole("alert")).toBeNull();
    await user.upload(picker, bytes("e", 1));
    expect(chipsOf()).toBe(MAX_ATTACHMENTS);
    expect(screen.getByRole("alert")).toHaveTextContent("at most 4 attachments");
  });

  it("accepts exactly 2 MiB of attachments in total and refuses one byte more", async () => {
    const user = userEvent.setup();
    const { calls } = setup();
    const picker = screen.getByLabelText("Attach files");
    await user.upload(picker, bytes("first", MAX_ATTACHMENT_BYTES - 10));
    await user.upload(picker, bytes("over", 11));
    expect(chipsOf()).toBe(1);
    expect(screen.getByRole("alert")).toHaveTextContent("2 MiB");
    await user.upload(picker, bytes("fits", 10));
    expect(chipsOf()).toBe(2);
    expect(screen.queryByRole("alert")).toBeNull();
    await user.click(screen.getByRole("button", { name: "Send" }));
    await waitFor(() => expect(calls.find(sent)).toBeDefined());
    expect(calls.find(sent)?.body).toMatchObject({
      attachments: [
        { name: "first", data_base64: btoa("\0".repeat(MAX_ATTACHMENT_BYTES - 10)) },
        { name: "fits", data_base64: btoa("\0".repeat(10)) },
      ],
    });
  });

  it("sends text of exactly 1 MiB of UTF-8 and refuses one byte more", async () => {
    const user = userEvent.setup();
    const { calls } = setup();
    const input = screen.getByRole("textbox", { name: "Message" });
    // "é" is two bytes in UTF-8, so the character count is half the byte limit.
    const limit = "é".repeat(MAX_TEXT_BYTES / 2);
    expect(new TextEncoder().encode(limit).length).toBe(MAX_TEXT_BYTES);
    fireEvent.change(input, { target: { value: `${limit}a` } });
    await user.click(screen.getByRole("button", { name: "Send" }));
    expect(screen.getByRole("alert")).toHaveTextContent("longer than 1 MiB");
    expect(calls.some(sent)).toBe(false);
    fireEvent.change(input, { target: { value: limit } });
    await user.click(screen.getByRole("button", { name: "Send" }));
    await waitFor(() => expect(calls.find(sent)?.body).toMatchObject({ text: limit }));
  });
});

describe("Composer run states", () => {
  function renderRunning(onStop: () => void) {
    const daemon = fakeClient(() => json(202, { id: "m", status: "queued" }));
    render(
      <ClientContext.Provider value={daemon.client}>
        <Composer sessionId="s1" running onStop={onStop} />
      </ClientContext.Provider>,
    );
    return daemon;
  }

  it("offers Stop while a run is active and the composer is empty", async () => {
    const user = userEvent.setup();
    let stopped = 0;
    const { calls } = renderRunning(() => stopped++);
    expect(screen.queryByRole("button", { name: "Send" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Queue" })).toBeNull();
    await user.click(screen.getByRole("button", { name: "Stop" }));
    expect(stopped).toBe(1);
    expect(calls).toHaveLength(0);
  });

  it("queues a follow-up typed during a run and keeps Stop available", async () => {
    const user = userEvent.setup();
    const { calls } = renderRunning(() => undefined);
    await user.type(screen.getByRole("textbox", { name: "Message" }), "also check prices");
    expect(screen.getByRole("button", { name: "Stop" })).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Queue" }));
    await waitFor(() =>
      expect(calls.find(sent)).toMatchObject({
        path: "/v1/sessions/s1/messages",
        body: { text: "also check prices", mode: "queue" },
      }),
    );
  });

  it("queues behind a pending ticket without a faint Send or a Stop", async () => {
    const user = userEvent.setup();
    const daemon = fakeClient(() => json(202, { id: "m", status: "queued" }));
    render(
      <ClientContext.Provider value={daemon.client}>
        <Composer sessionId="s1" waiting onStop={() => undefined} />
      </ClientContext.Provider>,
    );
    const field = screen.getByRole("textbox", { name: "Message" });
    expect(field).toHaveAttribute("placeholder", "Follow up");
    expect(screen.queryByRole("button", { name: "Send" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Stop" })).toBeNull();
    await user.type(field, "after that, summarize");
    await user.click(screen.getByRole("button", { name: "Queue" }));
    await waitFor(() =>
      expect(daemon.calls.find(sent)).toMatchObject({ body: { text: "after that, summarize", mode: "queue" } }),
    );
  });

  it("shows a read-only note instead of a field on a device paired without steer", () => {
    localStorage.setItem("openside.scopes", JSON.stringify(["read", "approve"]));
    try {
      setup();
      expect(screen.getByRole("note")).toHaveTextContent("can’t send messages");
      expect(screen.queryByRole("textbox", { name: "Message" })).toBeNull();
    } finally {
      localStorage.removeItem("openside.scopes");
    }
  });
});

describe("Composer Enter key", () => {
  const original = window.matchMedia;
  afterEach(() => {
    window.matchMedia = original;
  });

  const enter = (input: HTMLElement, init: KeyboardEventInit & { keyCode?: number }) =>
    fireEvent.keyDown(input, { key: "Enter", ...init });

  function pointer(kind: "fine" | "coarse" | "none") {
    window.matchMedia = ((query: string) =>
      ({
        matches: query === `(pointer: ${kind})`,
        media: query,
        addEventListener: () => undefined,
        removeEventListener: () => undefined,
      }) as unknown as MediaQueryList) as typeof window.matchMedia;
  }

  function typed(text: string) {
    const daemon = setup();
    const input = screen.getByRole("textbox", { name: "Message" });
    fireEvent.change(input, { target: { value: text } });
    return { daemon, input };
  }

  it("does not send the Enter that commits an IME composition, in either event order", async () => {
    pointer("fine");
    const { daemon, input } = typed("한글");
    enter(input, { isComposing: true, keyCode: 229 });
    // Safari fires compositionend first, so the commit key arrives with isComposing already false.
    enter(input, { isComposing: false, keyCode: 229 });
    expect(daemon.calls.find(sent)).toBeUndefined();
    enter(input, { keyCode: 13 });
    await waitFor(() => expect(daemon.calls.find(sent)).toMatchObject({ body: { text: "한글" } }));
  });

  it("sends on Enter without a pointer, as in headless or kiosk browsers", async () => {
    pointer("none");
    const { daemon, input } = typed("hello");
    enter(input, { keyCode: 13 });
    await waitFor(() => expect(daemon.calls.find(sent)).toMatchObject({ body: { text: "hello" } }));
  });

  it("keeps Enter as a newline on a touch screen unless Ctrl or Cmd is held", async () => {
    pointer("coarse");
    const { daemon, input } = typed("hello");
    enter(input, { keyCode: 13 });
    expect(daemon.calls.find(sent)).toBeUndefined();
    enter(input, { keyCode: 13, ctrlKey: true });
    await waitFor(() => expect(daemon.calls.find(sent)).toMatchObject({ body: { text: "hello" } }));
  });
});
