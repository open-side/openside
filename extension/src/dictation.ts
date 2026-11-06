import { ApiError, type Client } from "../../web/src/api/client";
import type { SttStatus } from "../../web/src/api/types";
import { getLocale, t as webT } from "../../web/src/i18n";
import { base64Of } from "./capture";

/*
 * The side panel's microphone: the web composer's dictation (web/src/components/useDictation.ts)
 * without React. The daemon transcribes a recorded clip (`POST /v1/stt`) whenever it has a
 * transcriber; only without one does Chrome's own recognizer get a chance. Same states, limits and
 * messages as the web composer.
 *
 * One difference: Chrome never shows the microphone prompt inside a side panel, so a panel without
 * the permission opens the extension's mic-access page in a tab to ask, and the person presses the
 * mic again once it is allowed there (the grant belongs to the extension's origin).
 */

export type DictationState = "checking" | "unavailable" | "idle" | "recording" | "transcribing";

export interface Dictation {
  readonly state: DictationState;
  /** Why the microphone is off, for its tooltip; null while it can be used. */
  readonly reason: string | null;
  toggle(): void;
  /** Reads the daemon's transcriber status again (a new pairing may change it). */
  refresh(): void;
  dispose(): void;
}

export interface DictationOptions {
  api: () => Promise<Client>;
  onText: (text: string) => void;
  onError: (message: string) => void;
  /** The microphone is not allowed for the extension yet and Chrome cannot ask from the panel. */
  onNeedsAccess: () => void;
  onChange: () => void;
}

interface SpeechRecognitionLike {
  lang: string;
  interimResults: boolean;
  continuous: boolean;
  onresult: ((event: { results: ArrayLike<ArrayLike<{ transcript: string }>> }) => void) | null;
  onerror: ((event: { error: string }) => void) | null;
  onend: (() => void) | null;
  start: () => void;
  stop: () => void;
}
type SpeechRecognitionCtor = new () => SpeechRecognitionLike;

function speechRecognition(): SpeechRecognitionCtor | null {
  const scope = globalThis as { SpeechRecognition?: SpeechRecognitionCtor; webkitSpeechRecognition?: SpeechRecognitionCtor };
  return scope.SpeechRecognition ?? scope.webkitSpeechRecognition ?? null;
}

const RECORDER_TYPES = ["audio/webm;codecs=opus", "audio/webm", "audio/ogg;codecs=opus", "audio/mp4"];

function microphoneError(cause: unknown): string {
  const name = cause instanceof DOMException ? cause.name : "";
  if (name === "NotAllowedError" || name === "SecurityError") return webT("thread.mic.denied");
  if (name === "NotFoundError" || name === "OverconstrainedError") return webT("thread.mic.noDevice");
  return webT("thread.mic.failed");
}

function transcribeError(cause: unknown): string {
  const code = cause instanceof ApiError ? cause.code : "";
  if (code === "no_speech") return webT("thread.mic.noSpeech");
  if (code === "audio_too_large") return webT("thread.mic.tooLong");
  if (code === "person_device_required") return webT("thread.mic.deviceOnly");
  if (code === "stt_unavailable") return webT("thread.mic.unavailable");
  if (cause instanceof ApiError && cause.status === 0) return webT("thread.composer.error.unreachable");
  return webT("thread.mic.failed");
}

/** "granted", "prompt" or "denied"; browsers without the query are treated as able to ask. */
async function microphonePermission(): Promise<PermissionState> {
  try {
    return (await navigator.permissions.query({ name: "microphone" as PermissionName })).state;
  } catch {
    return "prompt";
  }
}

export function createDictation(options: DictationOptions): Dictation {
  let status: SttStatus | null = null;
  let state: DictationState = "idle";
  let speechBroken = false;
  let stopCurrent: (() => void) | null = null;
  let live = true;
  let reads = 0;

  const canRecord = () => typeof MediaRecorder !== "undefined" && typeof navigator.mediaDevices?.getUserMedia === "function";
  const engine = (): "recorder" | "speech" | "none" | null => {
    if (status === null) return null;
    if (status.available && canRecord()) return "recorder";
    // An agent connection is refused dictation outright; the recognizer would only route around that.
    return status.reason !== "person_device_required" && speechRecognition() && !speechBroken ? "speech" : "none";
  };
  const shown = (): DictationState => (status === null ? "checking" : engine() === "none" ? "unavailable" : state);
  const set = (next: DictationState) => {
    state = next;
    if (live) options.onChange();
  };

  function refresh(): void {
    const read = ++reads;
    void options.api().then(api => api.sttStatus()).then(
      next => { if (live && read === reads) { status = next; options.onChange(); } },
      () => { if (live && read === reads) { status = { available: false, reason: "stt_unavailable", max_bytes: 0, max_seconds: 0 }; options.onChange(); } },
    );
  }

  async function record(limits: SttStatus): Promise<void> {
    let stream: MediaStream;
    try {
      stream = await navigator.mediaDevices.getUserMedia({ audio: true });
    } catch (cause) {
      options.onError(microphoneError(cause));
      return;
    }
    if (!live) { stream.getTracks().forEach(track => track.stop()); return; }
    const type = RECORDER_TYPES.find(candidate => MediaRecorder.isTypeSupported?.(candidate));
    let recorder: MediaRecorder;
    try {
      recorder = new MediaRecorder(stream, type ? { mimeType: type } : undefined);
    } catch {
      stream.getTracks().forEach(track => track.stop());
      options.onError(webT("thread.mic.failed"));
      return;
    }
    const chunks: Blob[] = [];
    let cancelled = false;
    recorder.ondataavailable = event => { if (event.data.size) chunks.push(event.data); };
    const limit = window.setTimeout(() => { if (recorder.state === "recording") recorder.stop(); }, limits.max_seconds * 1000);
    recorder.onstop = async () => {
      window.clearTimeout(limit);
      stream.getTracks().forEach(track => track.stop());
      stopCurrent = null;
      if (cancelled || !live) return;
      const mime = recorder.mimeType || type || "audio/webm";
      const clip = new Blob(chunks, { type: mime });
      if (clip.size === 0) { set("idle"); return; }
      if (clip.size > limits.max_bytes) { set("idle"); options.onError(webT("thread.mic.tooLong")); return; }
      set("transcribing");
      // Stop while transcribing drops the clip: the request is aborted and nothing lands in the box.
      const abort = new AbortController();
      const cancel = () => { abort.abort(); set("idle"); };
      stopCurrent = cancel;
      try {
        const audio = base64Of(new Uint8Array(await clip.arrayBuffer()));
        if (abort.signal.aborted) return;
        const transcript = await (await options.api()).transcribe({ audio_base64: audio, mime, language: getLocale() }, abort.signal);
        if (live && !abort.signal.aborted) options.onText(transcript.text);
      } catch (cause) {
        if (!live || abort.signal.aborted) return;
        const code = cause instanceof ApiError ? cause.code : "";
        if (code === "stt_unavailable" || code === "person_device_required") status = { ...limits, available: false, reason: code === "person_device_required" ? "person_device_required" : "stt_unavailable" };
        options.onError(transcribeError(cause));
      } finally {
        if (stopCurrent === cancel) stopCurrent = null;
        if (live && !abort.signal.aborted) set("idle");
      }
    };
    stopCurrent = () => {
      cancelled = !live;
      if (recorder.state === "recording") recorder.stop();
    };
    try {
      recorder.start();
    } catch {
      window.clearTimeout(limit);
      stopCurrent = null;
      stream.getTracks().forEach(track => track.stop());
      options.onError(webT("thread.mic.failed"));
      return;
    }
    set("recording");
  }

  function listen(): void {
    const Recognition = speechRecognition();
    if (!Recognition) return;
    const recognition = new Recognition();
    recognition.lang = document.documentElement.lang || getLocale();
    recognition.interimResults = false;
    recognition.continuous = true;
    let heard = "";
    recognition.onresult = event => { heard = Array.from(event.results, result => result[0]?.transcript ?? "").join(" "); };
    recognition.onerror = event => {
      if (event.error === "not-allowed") options.onError(webT("thread.mic.denied"));
      else if (event.error !== "aborted" && event.error !== "no-speech") {
        speechBroken = true;
        options.onError(webT("thread.mic.unavailable"));
      }
    };
    recognition.onend = () => {
      stopCurrent = null;
      if (!live) return;
      set("idle");
      if (heard.trim()) options.onText(heard.trim());
    };
    stopCurrent = () => recognition.stop();
    try {
      recognition.start();
    } catch {
      stopCurrent = null;
      speechBroken = true;
      options.onError(webT("thread.mic.unavailable"));
      options.onChange();
      return;
    }
    set("recording");
  }

  async function start(): Promise<void> {
    const kind = engine();
    if (!status || (kind !== "recorder" && kind !== "speech")) return;
    const permission = await microphonePermission();
    if (!live) return;
    if (permission === "denied") { options.onError(webT("thread.mic.denied")); return; }
    if (permission === "prompt") { options.onNeedsAccess(); return; }
    if (kind === "recorder") await record(status);
    else listen();
  }

  refresh();
  return {
    get state() { return shown(); },
    get reason() {
      if (shown() !== "unavailable") return null;
      return status?.reason === "person_device_required" ? webT("thread.mic.deviceOnly") : webT("thread.mic.unavailable");
    },
    toggle() {
      const current = shown();
      if (current === "recording" || current === "transcribing") { stopCurrent?.(); return; }
      if (current === "idle") void start();
    },
    refresh,
    dispose() {
      // Closing the panel throws a running clip away.
      live = false;
      stopCurrent?.();
    },
  };
}
