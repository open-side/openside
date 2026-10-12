import { ArrowUp, FileText, Plus, Square, X } from "lucide-react";
import { useEffect, useLayoutEffect, useRef, useState, type FormEvent, type KeyboardEvent, type ReactNode } from "react";
import { ApiError } from "../api/client";
import type { SkillSummary } from "../api/types";
import { newMessageId, useClient } from "../context";
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
const MAX_TITLE = 80;
// Eight 24px lines plus the 16px vertical padding (styles.css .composer-field textarea).
const MAX_INPUT_HEIGHT = 208;

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

/** A new conversation is named after its first message (the daemon would say "New session"). */
function titleFromMessage(text: string, fallback: string): string {
  const line = text.split("\n").find((part) => part.trim())?.trim() ?? "";
  if (!line) return fallback;
  return line.length > MAX_TITLE ? `${line.slice(0, MAX_TITLE - 1).trimEnd()}…` : line;
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
  /** New chat only: called with the created conversation id after the first message is sent. */
  onCreated?: (sessionId: string) => void;
  /** Called after every accepted message (the transcript scrolls back to the latest turn). */
  onSent?: () => void;
  /** Pills shown above the composer (model picker, browser preview). */
  tools?: ReactNode;
}

export function Composer({ sessionId, running = false, waiting = false, onStop, model, onCreated, onSent, tools }: ComposerProps) {
  const queueing = running || waiting;
  const client = useClient();
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

  function addFiles(list: FileList | null) {
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
    let created: string | null = null;
    try {
      const attachments = await Promise.all(
        files.map(async (pending) => ({ name: pending.file.name, data_base64: await base64(pending.file) })),
      );
      let target = sessionId;
      if (target === null) {
        const session = await client.createSession({
          title: titleFromMessage(body, files[0]?.file.name ?? "New chat"),
          model: model?.trim() || null,
        });
        target = created = session.id;
      }
      // An idle session starts a run with any mode; a running one gets the message after the
      // current run (steering is offered per queued message).
      await client.submitMessage(target, {
        message_id: newMessageId(),
        text: body,
        mode: "queue",
        ...(attachments.length ? { attachments } : {}),
      });
      setText("");
      files.forEach((pending) => pending.thumbnail && URL.revokeObjectURL(pending.thumbnail));
      setFiles([]);
      onSent?.();
    } catch (cause) {
      setError(cause instanceof ApiError ? cause.message : "Request failed");
    } finally {
      setBusy(false);
    }
    if (created !== null) onCreated?.(created);
  }

  function onKeyDown(event: KeyboardEvent<HTMLTextAreaElement>) {
    if (event.nativeEvent.isComposing) return;
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
    // Enter sends with a keyboard and mouse; on touch screens Enter inserts a newline.
    const finePointer = typeof window.matchMedia !== "function" || window.matchMedia("(pointer: fine)").matches;
    if (event.key === "Enter" && !event.shiftKey && (event.metaKey || event.ctrlKey || finePointer)) {
      event.preventDefault();
      void send();
    }
  }

  return (
    <div className="composer-dock">
      {error ? (
        <p role="alert" className="composer-error">
          {error}
        </p>
      ) : null}
      {tools ? <div className="composer-tools">{tools}</div> : null}
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
        <div className="composer-row">
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
            <Plus size={22} strokeWidth={ICON_STROKE} aria-hidden="true" />
          </IconButton>
          <label className="composer-field">
            <span className="sr-only">Message</span>
            <textarea
              ref={textarea}
              value={text}
              onChange={(e) => update(e.target.value, e.target.selectionStart ?? e.target.value.length)}
              onKeyDown={onKeyDown}
              rows={1}
              placeholder={queueing ? "Follow up" : "Message"}
              role="combobox"
              aria-expanded={open}
              aria-controls="skill-mentions"
              aria-autocomplete="list"
            />
          </label>
          <div className="composer-actions">
            {running && onStop ? (
              <button type="button" className="composer-send" onClick={onStop}>
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
