import { useCallback, useEffect, useRef, useState } from "react";
import { ApiError } from "../api/client";
import type { SttStatus } from "../api/types";
import { useClient } from "../context";
import { getLocale, t } from "../i18n";

export type DictationState = "checking" | "unavailable" | "idle" | "recording" | "transcribing";

export interface Dictation {
  state: DictationState;
  /** Why the microphone is off, for its tooltip; null while it can be used. */
  reason: string | null;
  toggle: () => void;
}

// Chrome's own recognizer, where the browser ships one. Its types are not in lib.dom.
interface SpeechRecognitionLike {
  lang: string;
  interimResults: boolean;
  continuous: boolean;
  onresult: ((event: { results: ArrayLike<ArrayLike<{ transcript: string }>> }) => void) | null;
  onerror: ((event: { error: string }) => void) | null;
  onend: (() => void) | null;
  start: () => void;
  stop: () => void;
  abort: () => void;
}
type SpeechRecognitionCtor = new () => SpeechRecognitionLike;

function speechRecognition(): SpeechRecognitionCtor | null {
  const scope = globalThis as { SpeechRecognition?: SpeechRecognitionCtor; webkitSpeechRecognition?: SpeechRecognitionCtor };
  return scope.SpeechRecognition ?? scope.webkitSpeechRecognition ?? null;
}

// Opus in WebM is what Chromium records; Safari only offers MP4. The daemon accepts any audio/*.
const RECORDER_TYPES = ["audio/webm;codecs=opus", "audio/webm", "audio/ogg;codecs=opus", "audio/mp4"];

/** A file or recording as standard base64, the encoding message attachments and clips travel in. */
export function blobBase64(blob: Blob): Promise<string> {
  const { promise, resolve, reject } = Promise.withResolvers<string>();
  const reader = new FileReader();
  reader.onload = () => resolve(String(reader.result).replace(/^data:[^,]*,/, ""));
  reader.onerror = () => reject(reader.error);
  reader.readAsDataURL(blob);
  return promise;
}

function microphoneError(cause: unknown): string {
  const name = cause instanceof DOMException ? cause.name : "";
  if (name === "NotAllowedError" || name === "SecurityError") return t("thread.mic.denied");
  if (name === "NotFoundError" || name === "OverconstrainedError") return t("thread.mic.noDevice");
  return t("thread.mic.failed");
}

/**
 * The composer's microphone. The daemon transcribes a recorded clip (`POST /v1/stt`) whenever it
 * has a transcriber; only without one does the browser's own recognizer get a chance, since many
 * Chromium builds ship it without a working speech service. Text is handed to `onText` once; the
 * clip itself is never kept.
 */
export function useDictation(onText: (text: string) => void, onError: (message: string) => void): Dictation {
  const client = useClient();
  const [status, setStatus] = useState<SttStatus | null>(null);
  const [state, setState] = useState<DictationState>("checking");
  const [speechBroken, setSpeechBroken] = useState(false);
  const stopRef = useRef<(() => void) | null>(null);
  const live = useRef(true);
  const handlers = useRef({ onText, onError });
  handlers.current = { onText, onError };

  useEffect(() => {
    live.current = true;
    client.sttStatus().then(
      (next) => live.current && setStatus(next),
      () => live.current && setStatus({ available: false, reason: "stt_unavailable", max_bytes: 0, max_seconds: 0 }),
    );
    return () => {
      live.current = false;
      stopRef.current?.();
    };
  }, [client]);

  const canRecord = typeof MediaRecorder !== "undefined" && typeof navigator.mediaDevices?.getUserMedia === "function";
  // An agent connection is refused dictation outright; the browser recognizer would only route
  // around that refusal, so it is offered solely when the daemon has no transcriber.
  const refused = status?.reason === "person_device_required";
  const engine =
    status === null ? null : status.available && canRecord ? "recorder" : !refused && speechRecognition() && !speechBroken ? "speech" : "none";
  const shown: DictationState = status === null ? "checking" : engine === "none" ? "unavailable" : state === "checking" ? "idle" : state;

  const record = useCallback(async () => {
    if (!status) return;
    let stream: MediaStream;
    try {
      stream = await navigator.mediaDevices.getUserMedia({ audio: true });
    } catch (cause) {
      handlers.current.onError(microphoneError(cause));
      return;
    }
    if (!live.current) {
      stream.getTracks().forEach((track) => track.stop());
      return;
    }
    const type = RECORDER_TYPES.find((candidate) => MediaRecorder.isTypeSupported?.(candidate));
    let recorder: MediaRecorder;
    try {
      recorder = new MediaRecorder(stream, type ? { mimeType: type } : undefined);
    } catch {
      // The browser offers no recorder for this stream: release the microphone and say so.
      stream.getTracks().forEach((track) => track.stop());
      handlers.current.onError(t("thread.mic.failed"));
      return;
    }
    const chunks: Blob[] = [];
    let cancelled = false;
    recorder.ondataavailable = (event) => event.data.size && chunks.push(event.data);
    const limit = window.setTimeout(() => recorder.state === "recording" && recorder.stop(), status.max_seconds * 1000);
    recorder.onstop = async () => {
      window.clearTimeout(limit);
      stream.getTracks().forEach((track) => track.stop());
      stopRef.current = null;
      if (cancelled || !live.current) return;
      const mime = recorder.mimeType || type || "audio/webm";
      const clip = new Blob(chunks, { type: mime });
      if (clip.size === 0) {
        setState("idle");
        return;
      }
      if (clip.size > status.max_bytes) {
        setState("idle");
        handlers.current.onError(t("thread.mic.tooLong"));
        return;
      }
      setState("transcribing");
      // Stop while transcribing, or the composer closing, drops the clip: the request is aborted,
      // the daemon frees its place, and nothing lands in the box afterwards.
      const abort = new AbortController();
      const cancel = () => {
        abort.abort();
        // The mic is free at once; a reply still on its way is ignored.
        if (live.current) setState("idle");
      };
      stopRef.current = cancel;
      try {
        const audio = await blobBase64(clip);
        if (abort.signal.aborted) return;
        const transcript = await client.transcribe({ audio_base64: audio, mime, language: getLocale() }, abort.signal);
        if (live.current && !abort.signal.aborted) handlers.current.onText(transcript.text);
      } catch (cause) {
        if (!live.current || abort.signal.aborted) return;
        const code = cause instanceof ApiError ? cause.code : "";
        // A device that may not use the person's transcriber hears the same from GET /v1/stt
        // (available: false); a stale status only learns it here.
        const refused = code === "stt_unavailable" || code === "person_device_required";
        if (refused)
          setStatus({
            ...status,
            available: false,
            reason: code === "person_device_required" ? "person_device_required" : "stt_unavailable",
          });
        // stt_busy (too many clips in flight) and stt_failed share the generic failure message.
        handlers.current.onError(
          code === "no_speech"
            ? t("thread.mic.noSpeech")
            : code === "audio_too_large"
              ? t("thread.mic.tooLong")
              : code === "person_device_required"
                ? t("thread.mic.deviceOnly")
                : code === "stt_unavailable"
                  ? t("thread.mic.unavailable")
                  : cause instanceof ApiError && cause.status === 0
                    ? t("thread.composer.error.unreachable")
                    : t("thread.mic.failed"),
        );
      } finally {
        // An aborted clip already handed the mic back, and a new recording may own it by now.
        if (stopRef.current === cancel) stopRef.current = null;
        if (live.current && !abort.signal.aborted) setState("idle");
      }
    };
    stopRef.current = () => {
      // Unmounting throws the clip away; only the person's own stop sends it.
      cancelled = !live.current;
      if (recorder.state === "recording") recorder.stop();
    };
    try {
      recorder.start();
    } catch {
      window.clearTimeout(limit);
      stopRef.current = null;
      stream.getTracks().forEach((track) => track.stop());
      handlers.current.onError(t("thread.mic.failed"));
      return;
    }
    setState("recording");
  }, [client, status]);

  const listen = useCallback(() => {
    const Recognition = speechRecognition();
    if (!Recognition) return;
    const recognition = new Recognition();
    recognition.lang = document.documentElement.lang || getLocale();
    recognition.interimResults = false;
    recognition.continuous = true;
    let heard = "";
    recognition.onresult = (event) => {
      heard = Array.from(event.results, (result) => result[0]?.transcript ?? "").join(" ");
    };
    recognition.onerror = (event) => {
      if (event.error === "not-allowed") handlers.current.onError(t("thread.mic.denied"));
      else if (event.error !== "aborted" && event.error !== "no-speech") {
        // No speech service behind the API (the usual case in Chromium builds without Google keys).
        setSpeechBroken(true);
        handlers.current.onError(t("thread.mic.unavailable"));
      }
    };
    recognition.onend = () => {
      stopRef.current = null;
      if (!live.current) return;
      setState("idle");
      if (heard.trim()) handlers.current.onText(heard.trim());
    };
    stopRef.current = () => recognition.stop();
    try {
      recognition.start();
    } catch {
      // Already listening elsewhere or no speech service: never leave the button stuck on Stop.
      stopRef.current = null;
      setSpeechBroken(true);
      handlers.current.onError(t("thread.mic.unavailable"));
      return;
    }
    setState("recording");
  }, []);

  const toggle = useCallback(() => {
    if (shown === "recording" || shown === "transcribing") {
      stopRef.current?.();
      return;
    }
    if (shown !== "idle") return;
    if (engine === "recorder") void record();
    else if (engine === "speech") listen();
  }, [engine, listen, record, shown]);

  const reason =
    shown !== "unavailable"
      ? null
      : status?.reason === "person_device_required"
        ? t("thread.mic.deviceOnly")
        : t("thread.mic.unavailable");
  return { state: shown, reason, toggle };
}
