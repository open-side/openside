import { ArrowUp, Eye, FileText, Plus, Square, X } from "lucide-react";
import { useEffect, useLayoutEffect, useRef, useState, type FormEvent, type KeyboardEvent, type ReactNode } from "react";
import { ApiError } from "../api/client";
import type { AttachmentUpload, BrowserSpace, SkillSummary } from "../api/types";
import { mayUse } from "../auth";
import { newMessageId, useClient } from "../context";
import { OutboxNotice } from "../pwa/OutboxNotice";
import { outbox } from "../pwa/outbox";
import type { ShareDraft } from "../pwa/share";
import { ICON_STROKE, IconButton } from "../ui";

// Same rules as the sidecar (agent/src/openside_agent/skills.py `_MENTION`): a mention starts
// a word and ends before whitespace or punctuation; `/a/b` and `/a.md` are paths, not mentions.
const MENTION = /(?:^|(?<=[\s([{"']))\/([a-z0-9]+(?:-[a-z0-9]+)*)(?=$|[\s)\]}"',;!?]|[.:](?:$|\s))/g;
const MAX_MENTIONS = 5;
// The daemon's limits (openside-proto MAX_ATTACHMENTS / MAX_ATTACHMENT_BYTES, sidecar
// MAX_USER_TEXT_BYTES); checked here so a large pick never gets read into memory.
export const MAX_ATTACHMENTS = 4;
export const MAX_ATTACHMENT_BYTES = 2 * 1024 * 1024;
export const MAX_TEXT_BYTES = 1024 * 1024;
/** The `/prefix` being typed right before the caret. */
const TYPING = /(?:^|[\s([{"'])\/([a-z0-9-]*)$/;
// Eight 24px lines plus the 16px vertical padding (styles.css .composer-field textarea).
const MAX_INPUT_HEIGHT = 208;
const PREWARM_EVERY_MS = 30_000;

interface PendingFile {
  key: string;
  file: File;
  /** Object URL for image thumbnails; revoked when the file leaves the list. */
  thumbnail: string | null;
}

function base64(file: File): Promise<string> {
  const { promise, resolve, reject } = Promise.withResolvers<string>();
  const reader = new FileReader();
  reader.onload = () => resolve(String(reader.result).replace(/^data:[^,]*,/, ""));
  reader.onerror = () => reject(reader.error);
  reader.readAsDataURL(file);
  return promise;
}

export function mentionedSkills(text: string, known: ReadonlySet<string>): string[] {
  const found: string[] = [];
  for (const match of text.matchAll(MENTION)) {
    const name = match[1] ?? "";
    if (known.has(name) && !found.includes(name)) found.push(name);
  }
  return found.slice(0, MAX_MENTIONS);
}

export interface ComposerProps {
  /** `null` on the new-chat screen: the first send creates the conversation. */
  sessionId: string | null;
  /** While a run is active, Send becomes Queue and an empty composer offers Stop. */
  running?: boolean;
  /** The run is paused on a ticket: messages queue behind it, but there is nothing to stop. */
  waiting?: boolean;
  onStop?: () => void;
  /** New chat only: model for the conversation the first message creates (empty = default). */
  model?: string;
  /** New chat only: where the created conversation browses (omitted = shared profile). */
  browserSpace?: BrowserSpace;
  /** New chat only: called with the created conversation id after the first message is sent. */
  onCreated?: (sessionId: string) => void;
  /** Called after every accepted message (the transcript scrolls back to the latest turn). */
  onSent?: () => void;
  /** Compact controls in the composer's bottom bar, after Add files (model picker, browser preview). */
  tools?: ReactNode;
  /** New chat only: content shared to the app (share target); applied once, then `onDraftApplied` fires. */
  draft?: ShareDraft | null;
  onDraftApplied?: () => void;
}

export function Composer({ sessionId, running = false, waiting = false, onStop, model, browserSpace, onCreated, onSent, tools, draft, onDraftApplied }: ComposerProps) {
  const queueing = running || waiting;
  const client = useClient();
  const warmed = useRef({ key: "", at: -Infinity });
  const [text, setText] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [query, setQuery] = useState<string | null>(null);
  const [candidates, setCandidates] = useState<SkillSummary[]>([]);
  const [active, setActive] = useState(0);
  const [known, setKnown] = useState<ReadonlySet<string>>(new Set());
  const [files, setFiles] = useState<PendingFile[]>([]);
  const textarea = useRef<HTMLTextAreaElement>(null);
  const picker = useRef<HTMLInputElement>(null);

  // Something was shared to the app: it becomes this composer's starting message.
  useEffect(() => {
    if (!draft) return;
    setText(draft.text);
    addFiles(draft.files);
    onDraftApplied?.();
    textarea.current?.focus();
    // eslint-disable-next-line react-hooks/exhaustive-deps -- applied once per draft id
  }, [draft?.id]);

  useEffect(() => {
    // Skill search is scoped to a session; a new chat has none until the first send.
    if (query === null || sessionId === null) return;
    let cancelled = false;
    const timer = setTimeout(() => {
      client
        .searchSkills(sessionId, query)
        .then((list) => {
          if (cancelled) return;
          setCandidates(list.skills);
          setActive(0);
          setKnown((current) => new Set([...current, ...list.skills.map((skill) => skill.name)]));
        })
        .catch(() => !cancelled && setCandidates([]));
    }, 120);
    return () => {
      cancelled = true;
      clearTimeout(timer);
    };
  }, [client, sessionId, query]);

  // Thumbnails of files still pending are released when the composer goes away.
  const pendingFiles = useRef(files);
  useEffect(() => {
    pendingFiles.current = files;
  }, [files]);
  useEffect(
    () => () => pendingFiles.current.forEach((pending) => pending.thumbnail && URL.revokeObjectURL(pending.thumbnail)),
    [],
  );

  // Grow with the text up to a cap; only past the cap does the field scroll (and show a bar).
  useLayoutEffect(() => {
    const element = textarea.current;
    if (!element) return;
    element.style.height = "auto";
    const height = element.scrollHeight;
    element.style.height = `${Math.min(height, MAX_INPUT_HEIGHT)}px`;
    element.style.overflowY = height > MAX_INPUT_HEIGHT ? "auto" : "hidden";
  }, [text]);

  const chips = mentionedSkills(text, known);
  const open = query !== null && candidates.length > 0;
  const hasInput = text.trim().length > 0 || files.length > 0;

  function update(value: string, caret: number) {
    setText(value);
    const typing = TYPING.exec(value.slice(0, caret));
    setQuery(typing ? (typing[1] ?? "") : null);
    if (!typing) setCandidates([]);
  }

  function choose(skill: SkillSummary) {
    const element = textarea.current;
    const caret = element?.selectionStart ?? text.length;
    const before = text.slice(0, caret).replace(/\/[a-z0-9-]*$/, `/${skill.name} `);
    const value = before + text.slice(caret);
    setText(value);
    setQuery(null);
    setCandidates([]);
    requestAnimationFrame(() => {
      element?.focus();
      element?.setSelectionRange(before.length, before.length);
    });
  }

  function removeMention(name: string) {
    const mention = new RegExp(`(?:^|(?<=[\\s([{"']))/${name}(?=$|[\\s)\\]}"',;!?]|[.:](?:$|\\s)) ?`, "g");
    setText((current) => current.replace(mention, ""));
  }

  function addFiles(list: ArrayLike<File> | null) {
    if (!list?.length) return;
    const picked = Array.from(list);
    if (files.length + picked.length > MAX_ATTACHMENTS) {
      setError(`A message can carry at most ${MAX_ATTACHMENTS} attachments.`);
      return;
    }
    const bytes = [...files.map((pending) => pending.file), ...picked].reduce((sum, file) => sum + file.size, 0);
    if (bytes > MAX_ATTACHMENT_BYTES) {
      setError("Attachments are limited to 2 MiB in total.");
      return;
    }
    setError(null);
    const added = picked.map((file) => ({
      key: newMessageId(),
      file,
      thumbnail: file.type.startsWith("image/") ? URL.createObjectURL(file) : null,
    }));
    setFiles((current) => [...current, ...added]);
  }

  async function send(event?: FormEvent) {
    event?.preventDefault();
    const body = text.trim();
    if ((!body && !files.length) || busy) return;
    if (new TextEncoder().encode(body).length > MAX_TEXT_BYTES) {
      setError("The message is longer than 1 MiB.");
      return;
    }
    setBusy(true);
    setError(null);
    // One id per send: a retry from the outbox after a lost response is deduplicated by the daemon.
    const messageId = newMessageId();
    let created: string | null = null;
    let target = sessionId;
    let attachments: AttachmentUpload[] = [];
    const newChat = {
      model: model?.trim() || null,
      ...(browserSpace === "isolated" ? { browser_space: browserSpace } : {}),
    };
    try {
      attachments = await Promise.all(
        files.map(async (pending) => ({ name: pending.file.name, data_base64: await base64(pending.file) })),
      );
      if (target === null) {
        const session = await client.createSession(newChat);
        target = created = session.id;
      }
      // An idle session starts a run with any mode; a running one gets the message after the
      // current run (steering is offered per queued message).
      await client.submitMessage(target, {
        message_id: messageId,
        text: body,
        mode: "queue",
        ...(attachments.length ? { attachments } : {}),
      });
      clearInput();
      onSent?.();
    } catch (cause) {
      if (cause instanceof ApiError && cause.status === 0) {
        // Unreachable daemon: keep the message (and the chat it creates) for the outbox to deliver.
        try {
          await outbox.enqueue({
            message_id: messageId,
            session_id: target,
            ...(target === null ? { create: newChat } : {}),
            text: body,
            attachments,
            queued_at: Date.now(),
          });
          clearInput();
        } catch {
          setError(cause.message);
        }
      } else setError(cause instanceof ApiError ? cause.message : "Request failed");
    } finally {
      setBusy(false);
    }
    if (created !== null) onCreated?.(created);
  }

  function clearInput() {
    setText("");
    files.forEach((pending) => pending.thumbnail && URL.revokeObjectURL(pending.thumbnail));
    setFiles([]);
  }

  // Focus means a message is coming: start the agent process and browser now instead of
  // after Send. Best effort, at most once per session per half minute.
  function prewarm() {
    const key = sessionId ?? "";
    const now = Date.now();
    if (warmed.current.key === key && now - warmed.current.at < PREWARM_EVERY_MS) return;
    warmed.current = { key, at: now };
    const request = sessionId === null ? client.prewarmBrowser() : client.prewarmSession(sessionId);
    request.catch(() => undefined);
  }

  function onKeyDown(event: KeyboardEvent<HTMLTextAreaElement>) {
    // Safari reports the Enter that commits an IME composition after compositionend, with
    // isComposing already false but keyCode 229.
    if (event.nativeEvent.isComposing || event.keyCode === 229) return;
    if (open) {
      if (event.key === "ArrowDown" || event.key === "ArrowUp") {
        event.preventDefault();
        const step = event.key === "ArrowDown" ? 1 : -1;
        setActive((index) => (index + step + candidates.length) % candidates.length);
        return;
      }
      const chosen = candidates[active];
      if ((event.key === "Enter" || event.key === "Tab") && chosen) {
        event.preventDefault();
        choose(chosen);
        return;
      }
      if (event.key === "Escape") {
        setQuery(null);
        setCandidates([]);
        return;
      }
    }
    // Enter sends with a keyboard and mouse; only a coarse (touch) pointer inserts a newline.
    // Pointer-less browsers (headless, kiosk) report (pointer: none) and still have a keyboard.
    const touch = typeof window.matchMedia === "function" && window.matchMedia("(pointer: coarse)").matches;
    if (event.key === "Enter" && !event.shiftKey && (event.metaKey || event.ctrlKey || !touch)) {
      event.preventDefault();
      void send();
    }
  }

  // A device paired without `steer` would only collect 403s; tell it what it can do instead.
  if (mayUse("steer") === false) {
    return (
      <div className="composer-dock">
        <p className="composer-readonly" role="note">
          <Eye size={16} strokeWidth={ICON_STROKE} aria-hidden="true" />
          This device can follow conversations but can’t send messages.
        </p>
      </div>
    );
  }

  return (
    <div className="composer-dock">
      <OutboxNotice sessionId={sessionId} />
      {error ? (
        <p role="alert" className="composer-error">
          {error}
        </p>
      ) : null}
      <form className="composer" onSubmit={send}>
        {chips.length || files.length ? (
          <ul className="composer-chips" aria-label="Message context">
            {chips.map((name) => (
              <li key={`s-${name}`} className="chip skill-chip">
                /{name}
                <button type="button" className="chip-remove" aria-label={`Remove skill ${name}`} onClick={() => removeMention(name)}>
                  <X size={14} strokeWidth={ICON_STROKE} aria-hidden="true" />
                </button>
              </li>
            ))}
            {files.map((pending) => (
              <li key={pending.key} className="attachment-chip">
                {pending.thumbnail ? (
                  <img src={pending.thumbnail} alt="" className="attachment-thumb" />
                ) : (
                  <span className="attachment-file">
                    <FileText size={20} strokeWidth={ICON_STROKE} aria-hidden="true" />
                    <span className="attachment-name">{pending.file.name}</span>
                  </span>
                )}
                {pending.thumbnail ? <span className="sr-only">{pending.file.name}</span> : null}
                <button
                  type="button"
                  className="chip-remove attachment-remove"
                  aria-label={`Remove attachment ${pending.file.name}`}
                  onClick={() => {
                    if (pending.thumbnail) URL.revokeObjectURL(pending.thumbnail);
                    setFiles((current) => current.filter((f) => f.key !== pending.key));
                  }}
                >
                  <X size={14} strokeWidth={ICON_STROKE} aria-hidden="true" />
                </button>
              </li>
            ))}
          </ul>
        ) : null}
        <label className="composer-field">
          <span className="sr-only">Message</span>
          <textarea
            ref={textarea}
            value={text}
            onChange={(e) => update(e.target.value, e.target.selectionStart ?? e.target.value.length)}
            onKeyDown={onKeyDown}
            onFocus={prewarm}
            rows={1}
            placeholder={queueing ? "Follow up" : "Message openside"}
            aria-autocomplete="list"
            aria-controls={open ? "skill-mentions" : undefined}
            aria-activedescendant={open && candidates[active] ? `skill-mention-${candidates[active].name}` : undefined}
          />
        </label>
        <div className="composer-bar">
          <input
            ref={picker}
            type="file"
            multiple
            className="sr-only"
            tabIndex={-1}
            aria-label="Attach files"
            onChange={(e) => {
              addFiles(e.target.files);
              e.target.value = "";
            }}
          />
          <IconButton label="Add files" className="composer-attach" onClick={() => picker.current?.click()}>
            <Plus size={20} strokeWidth={ICON_STROKE} aria-hidden="true" />
          </IconButton>
          <div className="composer-tools">{tools}</div>
          <div className="composer-actions">
            {running && onStop ? (
              <button type="button" className="composer-send composer-stop" onClick={onStop}>
                <Square size={14} fill="currentColor" strokeWidth={0} aria-hidden="true" />
                <span className="sr-only">Stop</span>
              </button>
            ) : null}
            {!queueing || hasInput ? (
              <button type="submit" className="composer-send" disabled={busy || !hasInput}>
                <ArrowUp size={20} strokeWidth={2.2} aria-hidden="true" />
                <span className="sr-only">{queueing ? "Queue" : "Send"}</span>
              </button>
            ) : null}
          </div>
        </div>
        {open ? (
          <ul id="skill-mentions" className="mention-list" role="listbox" aria-label="Skills">
            {candidates.map((skill, index) => (
              <li
                key={skill.name}
                id={`skill-mention-${skill.name}`}
                role="option"
                aria-selected={index === active}
                onMouseDown={(e) => {
                  e.preventDefault();
                  choose(skill);
                }}
              >
                <strong>/{skill.name}</strong> <span className="muted">{skill.description}</span>
              </li>
            ))}
          </ul>
        ) : null}
      </form>
    </div>
  );
}
