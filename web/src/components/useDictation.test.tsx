import { cleanup, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { ClientContext } from "../context";
import { getLocale } from "../i18n";
import { composerDaemon, json, type ComposerDaemonOptions } from "../test/composerDaemon";
import { Composer } from "./Composer";

const CLIP = "spoken words";

/** Records one clip of CLIP when stopped, as Chromium's recorder would. */
class FakeRecorder {
  static isTypeSupported = (type: string) => type === "audio/webm;codecs=opus";
  state: "inactive" | "recording" = "inactive";
  mimeType: string;
  ondataavailable: ((event: { data: Blob }) => void) | null = null;
  onstop: (() => void) | null = null;
  constructor(_stream: MediaStream, options?: { mimeType?: string }) {
    this.mimeType = options?.mimeType ?? "";
  }
  start() {
    this.state = "recording";
  }
  stop() {
    this.state = "inactive";
    this.ondataavailable?.({ data: new Blob([CLIP], { type: this.mimeType }) });
    this.onstop?.();
  }
}

const track = { stop: vi.fn() };
const getUserMedia = vi.fn<(constraints: MediaStreamConstraints) => Promise<MediaStream>>();

function mount(options: ComposerDaemonOptions = {}) {
  const daemon = composerDaemon(options);
  render(
    <ClientContext.Provider value={daemon.client}>
      <MemoryRouter>
        <Composer sessionId="s1" />
      </MemoryRouter>
    </ClientContext.Provider>,
  );
  return daemon;
}

function withRecorder() {
  vi.stubGlobal("MediaRecorder", FakeRecorder);
  Object.defineProperty(navigator, "mediaDevices", { configurable: true, value: { getUserMedia } });
}

beforeEach(() => {
  getUserMedia.mockReset();
  getUserMedia.mockResolvedValue({ getTracks: () => [track] } as unknown as MediaStream);
  track.stop.mockReset();
});

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
  Reflect.deleteProperty(navigator, "mediaDevices");
});

describe("dictation", () => {
  it("keeps an unusable microphone visible but off, saying why in its description and tooltip", async () => {
    const user = userEvent.setup();
    const { calls } = mount({ stt: { available: false, reason: "stt_unavailable" } });
    const reason = "Voice input needs an OpenAI API key in Settings › Models.";
    await waitFor(() => expect(screen.getByRole("button", { name: "Dictate" })).toHaveAttribute("aria-description", reason));
    const mic = screen.getByRole("button", { name: "Dictate" });
    expect(mic).toHaveAttribute("aria-disabled", "true");
    // aria-disabled rather than disabled, so the tooltip still answers hover.
    expect(mic).toBeEnabled();
    await user.hover(mic);
    expect(await screen.findByText(reason)).toBeInTheDocument();
    await user.click(mic);
    expect(calls.some((call) => call.method === "POST" && call.path === "/v1/stt")).toBe(false);
  });

  it("tells a device that may not use the person's transcriber that it is for their own devices", async () => {
    withRecorder();
    mount({ stt: { available: false, reason: "person_device_required" } });
    const reason = "Voice input works only from your own devices, not from agent connections.";
    await waitFor(() => expect(screen.getByRole("button", { name: "Dictate" })).toHaveAttribute("aria-description", reason));
    expect(screen.getByRole("button", { name: "Dictate" })).toHaveAttribute("aria-disabled", "true");
  });

  it("does not let the browser's recognizer route around a refused device", async () => {
    const start = vi.fn();
    vi.stubGlobal(
      "SpeechRecognition",
      class {
        lang = "";
        interimResults = false;
        continuous = false;
        onresult = null;
        onerror = null;
        onend = null;
        start = start;
        stop() {}
        abort() {}
      },
    );
    const user = userEvent.setup();
    mount({ stt: { available: false, reason: "person_device_required" } });
    await waitFor(() => expect(screen.getByRole("button", { name: "Dictate" })).toHaveAttribute("aria-disabled", "true"));
    await user.click(screen.getByRole("button", { name: "Dictate" }));
    expect(start).not.toHaveBeenCalled();
  });

  it("falls back to the browser's recognizer only when the daemon has no transcriber", async () => {
    const start = vi.fn();
    vi.stubGlobal(
      "SpeechRecognition",
      class {
        lang = "";
        interimResults = false;
        continuous = false;
        onresult = null;
        onerror = null;
        onend = null;
        start = start;
        stop() {}
        abort() {}
      },
    );
    const user = userEvent.setup();
    mount({ stt: { available: false, reason: "stt_unavailable" } });
    await waitFor(() => expect(screen.getByRole("button", { name: "Dictate" })).not.toHaveAttribute("aria-disabled", "true"));
    await user.click(screen.getByRole("button", { name: "Dictate" }));
    expect(start).toHaveBeenCalledTimes(1);
  });

  it("records, transcribes on stop and puts the text into the field", async () => {
    withRecorder();
    const user = userEvent.setup();
    const { calls } = mount({ transcript: { text: "book a table" } });
    const field = screen.getByRole("textbox", { name: "Message" });
    await waitFor(() => expect(screen.getByRole("button", { name: "Dictate" })).not.toHaveAttribute("aria-disabled"));
    await user.click(screen.getByRole("button", { name: "Dictate" }));
    const stop = await screen.findByRole("button", { name: "Stop dictating" });
    expect(stop).toHaveAttribute("aria-pressed", "true");
    expect(getUserMedia).toHaveBeenCalledWith({ audio: true });
    await user.click(stop);
    await waitFor(() => expect(field).toHaveValue("book a table"));
    expect(calls.find((call) => call.method === "POST" && call.path === "/v1/stt")?.body).toEqual({
      audio_base64: btoa(CLIP),
      mime: "audio/webm;codecs=opus",
      language: getLocale(),
    });
    expect(track.stop).toHaveBeenCalled();
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("says the microphone is blocked when the browser refuses it", async () => {
    withRecorder();
    getUserMedia.mockRejectedValue(new DOMException("denied", "NotAllowedError"));
    const user = userEvent.setup();
    mount();
    await waitFor(() => expect(screen.getByRole("button", { name: "Dictate" })).not.toHaveAttribute("aria-disabled"));
    await user.click(screen.getByRole("button", { name: "Dictate" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Microphone access is blocked.");
    expect(screen.getByRole("button", { name: "Dictate" })).toBeInTheDocument();
  });

  it("asks to try again when no speech was heard", async () => {
    withRecorder();
    const user = userEvent.setup();
    mount({ respond: (call) => (call.method === "POST" && call.path === "/v1/stt" ? json(422, { code: "no_speech", message: "x" }) : undefined) });
    await waitFor(() => expect(screen.getByRole("button", { name: "Dictate" })).not.toHaveAttribute("aria-disabled"));
    await user.click(screen.getByRole("button", { name: "Dictate" }));
    await user.click(await screen.findByRole("button", { name: "Stop dictating" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("No speech was heard. Try again.");
    expect(screen.getByRole("textbox", { name: "Message" })).toHaveValue("");
  });

  it("turns the microphone off when the daemon refuses this device after recording", async () => {
    withRecorder();
    const user = userEvent.setup();
    mount({ respond: (call) => (call.method === "POST" && call.path === "/v1/stt" ? json(403, { code: "person_device_required", message: "x" }) : undefined) });
    await waitFor(() => expect(screen.getByRole("button", { name: "Dictate" })).not.toHaveAttribute("aria-disabled"));
    await user.click(screen.getByRole("button", { name: "Dictate" }));
    await user.click(await screen.findByRole("button", { name: "Stop dictating" }));
    const message = "Voice input works only from your own devices, not from agent connections.";
    expect(await screen.findByRole("alert")).toHaveTextContent(message);
    await waitFor(() => expect(screen.getByRole("button", { name: "Dictate" })).toHaveAttribute("aria-description", message));
    expect(screen.getByRole("button", { name: "Dictate" })).toHaveAttribute("aria-disabled", "true");
  });

  it("releases the microphone and says so when the recorder cannot start", async () => {
    vi.stubGlobal(
      "MediaRecorder",
      class extends FakeRecorder {
        override start() {
          throw new DOMException("busy", "NotSupportedError");
        }
      },
    );
    Object.defineProperty(navigator, "mediaDevices", { configurable: true, value: { getUserMedia } });
    const user = userEvent.setup();
    mount();
    await waitFor(() => expect(screen.getByRole("button", { name: "Dictate" })).not.toHaveAttribute("aria-disabled"));
    await user.click(screen.getByRole("button", { name: "Dictate" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Couldn’t turn the recording into text. Try again.");
    expect(track.stop).toHaveBeenCalled();
    expect(screen.getByRole("button", { name: "Dictate" })).toBeInTheDocument();
  });

  it("drops the microphone it was still asking for when the composer goes away", async () => {
    withRecorder();
    const { promise, resolve } = Promise.withResolvers<MediaStream>();
    getUserMedia.mockReturnValue(promise);
    const user = userEvent.setup();
    const { calls } = mount();
    await waitFor(() => expect(screen.getByRole("button", { name: "Dictate" })).not.toHaveAttribute("aria-disabled"));
    await user.click(screen.getByRole("button", { name: "Dictate" }));
    cleanup();
    resolve({ getTracks: () => [track] } as unknown as MediaStream);
    await waitFor(() => expect(track.stop).toHaveBeenCalled());
    expect(calls.some((call) => call.method === "POST" && call.path === "/v1/stt")).toBe(false);
  });

  it("cancels a clip that is still being transcribed and frees the microphone at once", async () => {
    withRecorder();
    const reply = Promise.withResolvers<Response>();
    const user = userEvent.setup();
    const { calls } = mount({ respond: (call) => (call.method === "POST" && call.path === "/v1/stt" ? reply.promise : undefined) });
    await waitFor(() => expect(screen.getByRole("button", { name: "Dictate" })).not.toHaveAttribute("aria-disabled"));
    await user.click(screen.getByRole("button", { name: "Dictate" }));
    await user.click(await screen.findByRole("button", { name: "Stop dictating" }));
    const cancel = await screen.findByRole("button", { name: "Stop transcribing" });
    expect(cancel).toHaveAttribute("aria-description", "Transcribing…");
    expect(cancel).not.toHaveAttribute("aria-disabled");
    await user.click(cancel);
    expect(await screen.findByRole("button", { name: "Dictate" })).not.toHaveAttribute("aria-disabled");
    const post = calls.find((call) => call.method === "POST" && call.path === "/v1/stt");
    expect(post?.signal?.aborted).toBe(true);
    // A reply that arrives after the cancel is ignored.
    reply.resolve(json(200, { text: "too late" }));
    const settled = Promise.withResolvers<void>();
    setTimeout(settled.resolve, 20);
    await settled.promise;
    expect(screen.getByRole("textbox", { name: "Message" })).toHaveValue("");
    expect(screen.queryByRole("alert")).toBeNull();
  });

  it("aborts the transcription when the composer goes away mid-request", async () => {
    withRecorder();
    const reply = Promise.withResolvers<Response>();
    const user = userEvent.setup();
    const { calls } = mount({ respond: (call) => (call.method === "POST" && call.path === "/v1/stt" ? reply.promise : undefined) });
    await waitFor(() => expect(screen.getByRole("button", { name: "Dictate" })).not.toHaveAttribute("aria-disabled"));
    await user.click(screen.getByRole("button", { name: "Dictate" }));
    await user.click(await screen.findByRole("button", { name: "Stop dictating" }));
    await screen.findByRole("button", { name: "Stop transcribing" });
    const post = calls.find((call) => call.method === "POST" && call.path === "/v1/stt");
    expect(post?.signal?.aborted).toBe(false);
    cleanup();
    expect(post?.signal?.aborted).toBe(true);
    reply.resolve(json(200, { text: "too late" }));
  });
});
