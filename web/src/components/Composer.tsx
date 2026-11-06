import { ArrowUp, Crop, Eraser, Eye, FileText, Globe, Hand, MessageSquareWarning, RotateCcw, SquarePlus, Square, X } from "lucide-react";
import { useCallback, useContext, useEffect, useLayoutEffect, useRef, useState, type FormEvent, type KeyboardEvent, type ReactNode } from "react";
import { useNavigate } from "react-router";
import { ApiError, apiErrorMessage } from "../api/client";
import type { AttachedTab, AttachmentUpload, ChatConfirmation, CreateProjectRequest, CreateSessionRequest, McpServer, ModelCatalog, PolicyMode, Project, Session, SkillSummary } from "../api/types";
import { mayUse } from "../auth";
import { newMessageId, useClient } from "../context";
import { AgentTabsContext } from "../hooks/useAgentTabs";
import { useTeam } from "../hooks/useAgents";
import { openInBrowser, usePersonBrowser } from "../hooks/usePersonBrowser";
import { t } from "../i18n";
import { OutboxNotice } from "../pwa/OutboxNotice";
import { outbox } from "../pwa/outbox";
import type { ShareDraft } from "../pwa/share";
import { SETTINGS_FOOTER } from "../settings/nav";
import { outgoing } from "../state/outgoing";
import { cx, ICON_STROKE, IconButton } from "../ui";
import { ComposerContextMenu, CONTEXT_TYPING, contextItems, stepActive, type ContextCommand, type ContextItem, type ContextTrigger } from "./ComposerContextMenu";
import { ConfirmationMenu, ContextRing, EffortMenu, folderName, MicButton, PlusMenu, ProjectMenu, type MenuSide, type ProjectControls } from "./ComposerOptions";
import { ModelPicker } from "./ModelPicker";
import { leaveHandoff, takeHandoff } from "./composerHandoff";
import { createSessionRequest, DEFAULT_SESSION_OPTIONS, optionsPatch, sessionOptionsOf, type SessionOptions } from "./sessionOptions";
import { blobBase64, useDictation } from "./useDictation";

// Same rules as the sidecar (agent/src/features/skills.ts `MENTION_SOURCE`): a mention starts
// a word and ends before whitespace or punctuation; `/a/b` and `/a.md` are paths, not mentions.
const MENTION = /(?:^|(?<=[\s([{"']))\/([a-z0-9]+(?:-[a-z0-9]+)*)(?=$|[\s)\]}"',;!?]|[.:](?:$|\s))/g;
const MAX_MENTIONS = 5;
// The daemon's limits (openside-proto MAX_ATTACHMENTS / MAX_ATTACHMENT_BYTES, sidecar
// MAX_USER_TEXT_BYTES); checked here so a large pick never gets read into memory.
export const MAX_ATTACHMENTS = 4;
export const MAX_ATTACHMENT_BYTES = 2 * 1024 * 1024;
export const MAX_TEXT_BYTES = 1024 * 1024;
// Six 24px lines plus the vertical padding; composer.css lowers the cap on phones through the
// field's max-height, which this reads back.
const MAX_INPUT_HEIGHT = 160;
const PREWARM_EVERY_MS = 30_000;
const MAX_SKILL_ROWS = 8;
const FEEDBACK_URL = SETTINGS_FOOTER.find((link) => link.key === "feedback")?.url ?? "";

interface PendingFile {
  key: string;
  file: File;
  /** Object URL for image thumbnails; revoked when the file leaves the list. */
  thumbnail: string | null;
}

/** A person tab picked before the chat exists; it is attached once the chat is created. */
type PendingTab = Pick<AttachedTab, "tab_id" | "title" | "url">;

export function mentionedSkills(text: string, known: ReadonlySet<string>): string[] {
  const found: string[] = [];
  for (const match of text.matchAll(MENTION)) {
    const name = match[1] ?? "";
    if (known.has(name) && !found.includes(name)) found.push(name);
  }
  return found.slice(0, MAX_MENTIONS);
}

export interface ComposerProps {
  /** The chat this composer writes to; absent on Home, where sending creates a new chat. */
  sessionId?: string;
  /** While a run is active, Send becomes Queue and an empty composer offers Stop. */
  running?: boolean;
  /** The run is paused on a ticket: messages queue behind it, but there is nothing to stop. */
  waiting?: boolean;
  onStop?: () => void;
  /** The conversation's live connection is down; typing still works (the outbox holds messages). */
  reconnecting?: boolean;
  /** A request needs the person; shown as a button inside the message row, never a second row. */
  onShowPending?: () => void;
  /**
   * Called once a message is on its way. In a thread that is right away (the transcript shows its
   * delivery and scrolls back to it); on Home only after the daemon accepted it, with the chat
   * it created.
   */
  onSent?: (createdSessionId?: string) => void;
  /** Compact controls in the composer's bottom bar, after the confirmation chip (agents, files). */
  tools?: ReactNode;
  /** Content shared to the app (share target) or a picked starter; applied once, then `onDraftApplied` fires. */
  draft?: ShareDraft | null;
  onDraftApplied?: () => void;
  /** A quoted message to put above what is already typed; applied once, then `onQuoteApplied` fires. */
  quote?: { id: string; text: string } | null;
  onQuoteApplied?: () => void;
  /** The thread has no messages yet: it keeps the new-chat prompt instead of "Reply". */
  fresh?: boolean;
  /**
   * `home` starts a new chat (aside-gap-audit D1: a chat, never a persona's thread) with the
   * options picked in its toolbar; the toolbar shows once something is typed, under the pill.
   */
  variant?: "thread" | "home";
}

export function Composer({
  sessionId,
  running: runningProp = false,
  waiting: waitingProp = false,
  reconnecting = false,
  onStop,
  onShowPending,
  onSent,
  tools,
  draft,
  onDraftApplied,
  quote,
  onQuoteApplied,
  fresh = false,
  variant = "thread",
}: ComposerProps) {
  const home = variant === "home" || !sessionId;
  const running = !home && runningProp;
  const waiting = !home && waitingProp;
  const placeholder = reconnecting
    ? t("thread.composer.placeholder.reconnecting")
    : home || fresh
      ? t("thread.composer.placeholder.home")
      : waiting
        ? t("thread.composer.placeholder.waiting")
        : running
          ? t("thread.composer.placeholder.running")
          : t("thread.composer.placeholder.reply");
  const queueing = running || waiting;
  const side: MenuSide = home ? "bottom" : "top";
  const client = useClient();
  const navigate = useNavigate();
  const team = useTeam();
  const agentTabs = useContext(AgentTabsContext);
  const person = usePersonBrowser();
  const warmed = useRef({ key: "", at: -Infinity });
  const [text, setText] = useState("");
  const [busy, setBusy] = useState(false);
  // Home sends what was on screen when Send was pressed, then clears or hands it to the new chat;
  // anything typed, picked or dictated meanwhile would be lost, so Home's input holds still.
  const locked = home && busy;
  const lockedRef = useRef(locked);
  lockedRef.current = locked;
  const [error, setError] = useState<string | null>(null);
  const [known, setKnown] = useState<ReadonlySet<string>>(new Set());
  const [files, setFiles] = useState<PendingFile[]>([]);
  const textarea = useRef<HTMLTextAreaElement>(null);
  const picker = useRef<HTMLInputElement>(null);
  const form = useRef<HTMLFormElement>(null);
  const [context, setContext] = useState<{ trigger: ContextTrigger; query: string; below: boolean } | null>(null);
  const [contextActive, setContextActive] = useState(0);
  const [skills, setSkills] = useState<SkillSummary[]>([]);
  const [apps, setApps] = useState<McpServer[] | null>(null);
  const [catalog, setCatalog] = useState<ModelCatalog | null>(null);
  // What this device may ask for: unknown until read, and the daemon still has the last word.
  const [limits, setLimits] = useState<{ deviceMode: PolicyMode | null; agentClient: boolean }>({ deviceMode: null, agentClient: false });
  const [defaultConfirmation, setDefaultConfirmation] = useState<ChatConfirmation>("ask_first");
  const [projects, setProjects] = useState<Project[]>([]);
  // The settings document's `remote` is the daemon's verdict on this browser's local authority:
  // only the computer running openside opens the folder picker or makes a project.
  const [local, setLocal] = useState<boolean | null>(null);
  // Home: the new chat's options. Thread: the chat as the daemon reports it, plus a change on its way.
  const [picked, setPicked] = useState<SessionOptions>(DEFAULT_SESSION_OPTIONS);
  const [session, setSession] = useState<Session | null>(null);
  const [pending, setPending] = useState<Partial<SessionOptions>>({});
  const [attached, setAttached] = useState<PendingTab[]>([]);
  // Thread: tabs Home picked for this chat that could not be attached; the person retries or drops them.
  const [unattached, setUnattached] = useState<PendingTab[]>([]);
  // A chat created for a send whose message then failed: sending again reuses it.
  const started = useRef<{ request: string; id: string } | null>(null);
  const [touched, setTouched] = useState(false);

  const options: SessionOptions | null = home
    ? { ...picked, confirmation: picked.confirmation ?? defaultConfirmation }
    : session
      ? { ...sessionOptionsOf(session), ...pending }
      : null;
  const agentHome = !home && (team.agents ?? []).some((agent) => agent.home_session_id === sessionId);

  useEffect(() => {
    let live = true;
    client.modelCatalog().then(
      (next) => live && setCatalog(next),
      () => undefined,
    );
    client.settings().then(
      (settings) => live && setLimits((current) => ({ ...current, deviceMode: settings.policy_mode })),
      () => undefined,
    );
    client.myDevice().then(
      (me) => live && setLimits((current) => ({ ...current, agentClient: me.kind === "agent_client" })),
      () => undefined,
    );
    client.preferences().then(
      (preferences) => live && setDefaultConfirmation(preferences.confirmation),
      () => undefined,
    );
    client.listProjects().then(
      (list) => live && setProjects(list.projects),
      () => undefined,
    );
    client.settingsConfig().then(
      (document) => live && setLocal(!document.remote),
      () => live && setLocal(false),
    );
    return () => {
      live = false;
    };
  }, [client]);

  // Option PATCHes and session reads share one queue, so every full-Session answer arrives in the
  // order it was asked for and a read started before a change can never land after it. Each
  // field's optimistic value is dropped only by the answer to the newest change of that field.
  const optionsQueue = useRef<Promise<void>>(Promise.resolve());
  const optionsGeneration = useRef(0);
  const fieldGeneration = useRef<Partial<Record<keyof SessionOptions, number>>>({});

  // The chat's options and context use: read on arrival and again whenever a run starts or settles,
  // since a finished turn is what moves the context ring.
  useEffect(() => {
    if (home || !sessionId) return;
    let live = true;
    optionsQueue.current = optionsQueue.current.then(() =>
      client.session(sessionId).then(
        (next) => {
          if (live) setSession(next);
        },
        () => undefined,
      ),
    );
    return () => {
      live = false;
    };
  }, [client, home, sessionId, running]);

  useEffect(() => {
    if (home || !sessionId) return;
    let live = true;
    client.attachedTabs(sessionId).then(
      (list) => live && setAttached(list.tabs),
      () => undefined,
    );
    return () => {
      live = false;
    };
  }, [client, home, sessionId]);

  function changeOptions(patch: Partial<SessionOptions>) {
    setTouched(true);
    if (home) {
      // The chat is being created with the options on screen at Send; a later pick would be lost.
      if (lockedRef.current) return;
      setPicked((current) => ({ ...current, ...patch }));
      return;
    }
    if (!sessionId || !session) return;
    const generation = ++optionsGeneration.current;
    const fields = Object.keys(patch) as (keyof SessionOptions)[];
    for (const field of fields) fieldGeneration.current[field] = generation;
    setPending((current) => ({ ...current, ...patch }));
    const settle = () =>
      setPending((current) => {
        const next = { ...current };
        for (const field of fields) if (fieldGeneration.current[field] === generation) delete next[field];
        return next;
      });
    const target = sessionId;
    // Where the chat works has its own route; both fields always travel together.
    const folder = "projectId" in patch || "workspace" in patch;
    optionsQueue.current = optionsQueue.current.then(() =>
      (folder ? client.setSessionFolder(target, { project_id: patch.projectId ?? null, workspace: patch.workspace ?? null }) : client.updateSession(target, optionsPatch(patch))).then(
        (next) => {
          setSession(next);
          settle();
        },
        (cause: unknown) => {
          settle();
          // A refusal the app can explain (a device limit) says so; anything else is the generic retry.
          setError((cause instanceof ApiError && apiErrorMessage(cause.code)) || t(folder ? "thread.project.failed" : "thread.composer.error.optionsFailed"));
        },
      ),
    );
  }

  /** Makes a project on the computer running openside, from a picked folder or from scratch, and moves the chat into it. */
  async function createProject(from: "folder" | "scratch") {
    if (lockedRef.current) return;
    setError(null);
    try {
      let request: CreateProjectRequest = { name: t("thread.project.newName") };
      if (from === "folder") {
        const { path } = await client.pickFolder();
        if (!path || lockedRef.current) return;
        // The picker and the daemon both canonicalise, so a folder that already is a project matches its root.
        const known = projects.find((project) => project.root === path);
        if (known) {
          changeOptions({ projectId: known.id, workspace: null });
          return;
        }
        request = { name: folderName(path), root: path };
      }
      const project = await client.createProject(request);
      setProjects((current) => [...current.filter((other) => other.id !== project.id), project].sort((a, b) => a.name.localeCompare(b.name) || a.id.localeCompare(b.id)));
      if (!lockedRef.current) changeOptions({ projectId: project.id, workspace: null });
    } catch (cause) {
      setError((cause instanceof ApiError && apiErrorMessage(cause.code)) || t("thread.project.failed"));
    }
  }

  /** Gives the chat a folder picked on the computer running openside, outside any project. */
  async function attachFolder() {
    if (lockedRef.current) return;
    setError(null);
    try {
      const { path } = await client.pickFolder();
      if (path && !lockedRef.current) changeOptions({ projectId: null, workspace: path });
    } catch (cause) {
      setError((cause instanceof ApiError && apiErrorMessage(cause.code)) || t("thread.project.failed"));
    }
  }

  const projectControls: ProjectControls = {
    projects,
    local,
    onPick: (projectId) => {
      if (!lockedRef.current) changeOptions({ projectId, workspace: null });
    },
    onCreate: (from) => void createProject(from),
    onAttachFolder: () => void attachFolder(),
  };

  const insertText = useCallback((spoken: string) => {
    const element = textarea.current;
    if (lockedRef.current) return;
    setText((current) => {
      const caret = element && document.activeElement === element ? element.selectionStart : current.length;
      const before = current.slice(0, caret);
      const after = current.slice(caret);
      const joined = `${before}${before && !/\s$/.test(before) ? " " : ""}${spoken}${after && !/^\s/.test(after) ? " " : ""}${after}`;
      requestAnimationFrame(() => {
        const end = joined.length - after.length;
        element?.focus();
        element?.setSelectionRange(end, end);
      });
      return joined;
    });
  }, []);
  const dictation = useDictation(insertText, setError);

  // Something was shared to the app: it becomes this composer's starting message.
  useEffect(() => {
    if (!draft) return;
    setText(draft.text);
    addFiles(draft.files);
    onDraftApplied?.();
    textarea.current?.focus();
    // eslint-disable-next-line react-hooks/exhaustive-deps -- applied once per draft id
  }, [draft?.id]);

  // Home created this chat but could not attach every picked tab, so it sent nothing and left
  // the words, files and failed tabs here.
  useEffect(() => {
    if (home || !sessionId) return;
    const handoff = takeHandoff(sessionId);
    if (!handoff) return;
    setText(handoff.text);
    addFiles(handoff.files);
    setUnattached(handoff.tabs);
    setError(handoff.error);
    // eslint-disable-next-line react-hooks/exhaustive-deps -- taken once per chat
  }, [home, sessionId]);

  // A quoted message goes above whatever is already typed; the caret waits under the quote.
  useEffect(() => {
    if (!quote) return;
    setText((current) => quote.text + current);
    onQuoteApplied?.();
    requestAnimationFrame(() => {
      textarea.current?.focus();
      textarea.current?.setSelectionRange(quote.text.length, quote.text.length);
    });
    // eslint-disable-next-line react-hooks/exhaustive-deps -- applied once per quote id
  }, [quote?.id]);

  const contextQuery = context?.query ?? null;
  useEffect(() => {
    if (contextQuery === null) return;
    let cancelled = false;
    const timer = setTimeout(() => {
      client
        .searchSkills(sessionId, contextQuery)
        .then((list) => {
          if (cancelled) return;
          setSkills(list.skills.slice(0, MAX_SKILL_ROWS));
          setKnown((current) => new Set([...current, ...list.skills.map((skill) => skill.name)]));
        })
        .catch(() => !cancelled && setSkills([]));
    }, 120);
    return () => {
      cancelled = true;
      clearTimeout(timer);
    };
  }, [client, sessionId, contextQuery]);

  // Apps are read the first time `@` opens; the list rarely changes while the page is open.
  const wantsApps = context?.trigger === "@" && apps === null;
  useEffect(() => {
    if (!wantsApps) return;
    client.mcpServers().then(
      (list) => setApps(list.servers),
      () => setApps([]),
    );
  }, [client, wantsApps]);

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
  const fit = useCallback(() => {
    const element = textarea.current;
    if (!element) return;
    element.style.height = "auto";
    const height = element.scrollHeight;
    const cap = Number.parseFloat(getComputedStyle(element).maxHeight) || MAX_INPUT_HEIGHT;
    element.style.height = `${Math.min(height, cap)}px`;
    element.style.overflowY = height > cap ? "auto" : "hidden";
  }, []);
  useLayoutEffect(fit, [text, fit]);
  // The same text wraps differently once the field narrows (the viewport turns) or a web font
  // replaces the fallback, so a height fitted earlier would cut the new last line in half. Only
  // width changes refit: the height is ours to set.
  useEffect(() => {
    const element = textarea.current;
    if (!element) return;
    let width = element.clientWidth;
    const observer =
      typeof ResizeObserver === "undefined"
        ? null
        : new ResizeObserver(() => {
            if (element.clientWidth === width) return;
            width = element.clientWidth;
            fit();
          });
    observer?.observe(element);
    const fonts = typeof document.fonts?.addEventListener === "function" ? document.fonts : null;
    fonts?.addEventListener("loadingdone", fit);
    return () => {
      observer?.disconnect();
      fonts?.removeEventListener("loadingdone", fit);
    };
  }, [fit]);

  const chips = mentionedSkills(text, known);
  const hasInput = text.trim().length > 0 || files.length > 0;

  /**
   * Starts over in an empty chat with this chat's options: the nearest openside has to clearing one.
   * A folder of the chat's own can only be named from the computer running openside, so elsewhere
   * the new chat starts without it; a project carries over everywhere.
   */
  async function startOver() {
    try {
      const current = options ?? DEFAULT_SESSION_OPTIONS;
      const created = await client.createSession(createSessionRequest(local ? current : { ...current, workspace: null }));
      navigate(`/sessions/${encodeURIComponent(created.id)}`);
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : t("thread.composer.error.startFailed"));
    }
  }

  const commands: ContextCommand[] = [
    ...(home
      ? []
      : [
          { id: "clear", label: t("thread.composer.command.clear"), detail: t("thread.composer.command.clearDetail"), icon: Eraser, run: () => void startOver() },
          // openside summarizes long chats on its own; there is no manual compaction to ask for.
          { id: "compact", label: t("thread.composer.command.compact"), detail: t("thread.composer.command.compactDetail"), icon: Crop, disabled: true },
        ]),
    { id: "feedback", label: t("thread.composer.command.feedback"), detail: t("thread.composer.command.feedbackDetail"), icon: MessageSquareWarning, run: () => void openInBrowser(client, FEEDBACK_URL) },
    ...(home ? [] : [{ id: "new", label: t("thread.composer.command.new"), detail: t("thread.composer.command.newDetail"), icon: SquarePlus, run: () => navigate("/") }]),
  ];
  const contextRows = context
    ? contextItems({
        trigger: context.trigger,
        query: context.query,
        chatTabs: sessionId ? agentTabs.filter((tab) => tab.owner_session_id === sessionId) : [],
        agentTabs,
        personConnected: person.connected,
        personTabs: person.tabs,
        personGroups: person.groups,
        commands,
        skills,
        apps: apps ?? [],
      })
    : [];
  const contextOpen = contextRows.length > 0;
  const shownActive = Math.min(contextActive, contextRows.length - 1);
  // A disabled row (Compact) can come first; Enter would then do nothing, so the highlight skips it.
  const activeRow = !contextOpen ? 0 : contextRows[shownActive]?.disabled ? stepActive(contextRows, shownActive, 1) : shownActive;

  function update(value: string, caret: number) {
    if (locked) return;
    setText(value);
    const typing = CONTEXT_TYPING.exec(value.slice(0, caret));
    if (!typing) {
      setContext(null);
      return;
    }
    const trigger = typing[1] as ContextTrigger;
    const query = typing[2] ?? "";
    // Opens on the side with room: above a thread's card unless it sits too close to the top.
    const top = form.current?.getBoundingClientRect().top ?? Infinity;
    if (context?.query !== query || context.trigger !== trigger) setContextActive(0);
    setContext({ trigger, query, below: home || top < 256 });
  }

  function replaceTyped(replacement: string) {
    const element = textarea.current;
    const caret = element?.selectionStart ?? text.length;
    const before = text.slice(0, caret).replace(/[@/][^\s@/]*$/, replacement);
    setText(before + text.slice(caret));
    requestAnimationFrame(() => {
      element?.focus();
      element?.setSelectionRange(before.length, before.length);
    });
  }

  async function attachTabs(ids: readonly number[]) {
    if (home || !sessionId) {
      if (locked) return;
      const tabs = person.tabs.filter((tab) => ids.includes(tab.tab_id));
      setAttached((current) => [...current, ...tabs.filter((tab) => !current.some((known) => known.tab_id === tab.tab_id))]);
      return;
    }
    // A group pick is several attachments: keep each one that landed and list each refusal as a
    // failed chip, so a retry only asks again for what is missing.
    const results = await Promise.allSettled(ids.map((id) => client.attachTab(sessionId, id)));
    const added = results.flatMap((result) => (result.status === "fulfilled" ? [result.value] : []));
    const failedIds = ids.filter((_, index) => results[index]?.status === "rejected");
    setAttached((current) => [...current.filter((tab) => !added.some((next) => next.tab_id === tab.tab_id)), ...added]);
    setUnattached((current) => {
      const kept = current.filter((tab) => !added.some((next) => next.tab_id === tab.tab_id));
      const known = (id: number) => kept.find((tab) => tab.tab_id === id) ?? person.tabs.find((tab) => tab.tab_id === id);
      const fresh = failedIds.flatMap((id) => (kept.some((tab) => tab.tab_id === id) ? [] : [known(id) ?? { tab_id: id, title: "", url: "" }]));
      return [...kept, ...fresh.map(({ tab_id, title, url }) => ({ tab_id, title, url }))];
    });
    if (!failedIds.length) return true;
    const cause = results.find((result) => result.status === "rejected")?.reason as unknown;
    setError((cause instanceof ApiError && apiErrorMessage(cause.code)) || t("thread.composer.error.attachFailed"));
    return false;
  }

  async function retryAttach(tab: PendingTab) {
    if (await attachTabs([tab.tab_id])) setError(null);
  }

  function detach(tab: PendingTab) {
    if (locked) return;
    setAttached((current) => current.filter((known) => known.tab_id !== tab.tab_id));
    if (!home && sessionId) client.detachTab(sessionId, tab.tab_id).catch(() => undefined);
  }

  function pickContext(item: ContextItem) {
    if (item.disabled || locked) return;
    replaceTyped(item.insert ? `${item.insert} ` : "");
    setContext(null);
    if (item.attach?.length) void attachTabs(item.attach);
    item.run?.();
  }

  function removeMention(name: string) {
    if (locked) return;
    const mention = new RegExp(`(?:^|(?<=[\\s([{"']))/${name}(?=$|[\\s)\\]}"',;!?]|[.:](?:$|\\s)) ?`, "g");
    setText((current) => current.replace(mention, ""));
  }

  function addFiles(list: ArrayLike<File> | null) {
    if (!list?.length || locked) return;
    const picked = Array.from(list);
    if (files.length + picked.length > MAX_ATTACHMENTS) {
      setError(t("thread.composer.error.tooManyAttachments", { max: MAX_ATTACHMENTS }));
      return;
    }
    const bytes = [...files.map((pending) => pending.file), ...picked].reduce((sum, file) => sum + file.size, 0);
    if (bytes > MAX_ATTACHMENT_BYTES) {
      setError(t("thread.composer.error.attachmentsTooLarge"));
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
      setError(t("thread.composer.error.textTooLong"));
      return;
    }
    setBusy(true);
    setError(null);
    // One id per send: a retry from the outbox after a lost response is deduplicated by the daemon.
    const messageId = newMessageId();
    let attachments: AttachmentUpload[];
    try {
      attachments = await Promise.all(files.map(async (pending) => ({ name: pending.file.name, data_base64: await blobBase64(pending.file) })));
    } catch {
      setBusy(false);
      setError(t("thread.composer.error.fileUnreadable"));
      return;
    }
    // The conversation shows this message's delivery itself: Sending, Sent, or Not sent with Retry.
    // Home has no conversation on screen, so it keeps the words until they are safe.
    if (!home) {
      clearInput();
      onSent?.();
      setBusy(false);
    }
    let target = sessionId;
    if (home) {
      // `options` carries the confirmation default from Preferences; the chip shows it, so the
      // chat must be created with it rather than the daemon's own default.
      const created = options ? await startChat(createSessionRequest(options)) : null;
      if (created === null) return;
      target = created;
      // A picked tab is context the person asked for. If one cannot be attached, nothing is sent:
      // the new chat opens with the words, files and failed tabs so the person decides.
      const results = await Promise.allSettled(attached.map((tab) => client.attachTab(created, tab.tab_id)));
      const failed = attached.filter((_, index) => results[index]?.status === "rejected");
      if (failed.length) {
        const cause = results.find((result) => result.status === "rejected")?.reason as unknown;
        leaveHandoff(created, {
          text,
          files: files.map((pending) => pending.file),
          tabs: failed,
          error: (cause instanceof ApiError && apiErrorMessage(cause.code)) || t("thread.composer.error.attachFailed"),
        });
        setBusy(false);
        resetHome();
        onSent?.(created);
        return;
      }
    }
    if (!target) return;
    const failure = await outgoing.submit(client, { message_id: messageId, session_id: target, text: body, attachments });
    let kept = failure === null;
    if (failure?.status === 0) {
      // Unreachable daemon: the outbox delivers it later, and its notice replaces the bubble.
      try {
        await outbox.enqueue({ message_id: messageId, session_id: target, text: body, attachments, queued_at: Date.now() });
        outgoing.forget(messageId);
        kept = true;
      } catch {
        // Storage is unavailable too; the Not sent bubble stays and offers Retry.
      }
    }
    if (!home) return;
    setBusy(false);
    if (kept) {
      resetHome();
      onSent?.(target);
    } else {
      // The words are still here to send again, so the thread must not hold a Not sent copy too.
      outgoing.forget(messageId);
      setError(failure?.status === 0 ? t("thread.composer.error.unreachable") : (failure?.message ?? t("thread.composer.error.sendFailed")));
    }
  }

  /** Home starts over for the next chat once this one has its message or its handoff. */
  function resetHome() {
    clearInput();
    started.current = null;
    setPicked(DEFAULT_SESSION_OPTIONS);
    setAttached([]);
    setTouched(false);
  }

  /**
   * Creates the new chat, or reuses the one an earlier failed send created with the same options.
   * `null` when the daemon refused or could not be reached: the error is shown and the words and
   * options stay. Creation is never queued offline, because a replay after a lost response would
   * start a second chat.
   */
  async function startChat(request: CreateSessionRequest): Promise<string | null> {
    const key = JSON.stringify(request);
    if (started.current?.request === key) return started.current.id;
    try {
      const created = await client.createSession(request);
      started.current = { request: key, id: created.id };
      return created.id;
    } catch (cause) {
      setBusy(false);
      const offline = cause instanceof ApiError && cause.status === 0;
      setError(offline ? t("thread.composer.error.startOffline") : cause instanceof Error ? cause.message : t("thread.composer.error.startFailed"));
      return null;
    }
  }

  function clearInput() {
    setText("");
    files.forEach((pending) => pending.thumbnail && URL.revokeObjectURL(pending.thumbnail));
    setFiles([]);
  }

  // Focus means a message is coming: start the agent process and browser now instead of
  // after Send. Best effort, at most once per chat per half minute; Home has no chat yet.
  function prewarm() {
    if (home || !sessionId) return;
    const now = Date.now();
    if (warmed.current.key === sessionId && now - warmed.current.at < PREWARM_EVERY_MS) return;
    warmed.current = { key: sessionId, at: now };
    client.prewarmSession(sessionId).catch(() => undefined);
  }

  function onKeyDown(event: KeyboardEvent<HTMLTextAreaElement>) {
    // Safari reports the Enter that commits an IME composition after compositionend, with
    // isComposing already false but keyCode 229.
    if (event.nativeEvent.isComposing || event.keyCode === 229) return;
    if (contextOpen) {
      if (event.key === "ArrowDown" || event.key === "ArrowUp") {
        event.preventDefault();
        setContextActive(stepActive(contextRows, activeRow, event.key === "ArrowDown" ? 1 : -1));
        return;
      }
      const row = contextRows[activeRow];
      if ((event.key === "Enter" || event.key === "Tab") && row && !row.disabled) {
        event.preventDefault();
        pickContext(row);
        return;
      }
      if (event.key === "Escape") {
        event.preventDefault();
        setContext(null);
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
          {t("thread.composer.readOnly")}
        </p>
      </div>
    );
  }

  const stop = Boolean(running && onStop) && !hasInput;
  const sendCircle = hasInput || stop;
  const showBar = !home || hasInput || touched;

  const bar = (
    <div className="composer-bar">
      <input
        ref={picker}
        type="file"
        multiple
        className="sr-only"
        tabIndex={-1}
        aria-label={t("thread.composer.attachFiles")}
        onChange={(e) => {
          addFiles(e.target.files);
          e.target.value = "";
        }}
      />
      <div className="composer-bar-start">
        <PlusMenu options={options ?? DEFAULT_SESSION_OPTIONS} existing={!home} deviceLimit={limits.deviceMode} onChange={changeOptions} onUpload={() => picker.current?.click()} project={projectControls} side={side} />
        {home && options ? <ProjectMenu options={options} project={projectControls} side={side} /> : null}
        {options ? <ConfirmationMenu value={options.confirmation ?? defaultConfirmation} onChange={changeOptions} side={side} /> : null}
        {home ? null : <div className="composer-tools">{tools}</div>}
      </div>
      <div className="composer-actions">
        {options ? (
          <>
            <ModelPicker
              catalog={catalog}
              value={options.model}
              fallback={catalog?.default_model ?? null}
              onChange={(model) => changeOptions({ model })}
              disabled={agentHome}
              title={agentHome ? t("thread.model.homeFixed") : undefined}
              side={side}
            />
            <EffortMenu
              options={options}
              defaultEffort={
                catalog?.providers.flatMap((provider) => provider.models).find((model) => model.id === (options.model ?? catalog.default_model))?.default_effort ?? null
              }
              existing={!home}
              agentClient={limits.agentClient}
              onChange={changeOptions}
              side={side}
            />
          </>
        ) : null}
        {home ? null : <ContextRing context={session?.context} />}
        {onShowPending ? (
          <IconButton label={t("thread.composer.showPending")} className="composer-needs" onClick={onShowPending}>
            <Hand size={18} strokeWidth={ICON_STROKE} aria-hidden="true" />
          </IconButton>
        ) : null}
        {sendCircle ? <MicButton dictation={dictation} variant="glyph" /> : null}
        {stop ? (
          <button type="button" className="composer-send composer-stop" title={t("thread.composer.stopTask")} onClick={onStop}>
            <Square fill="currentColor" strokeWidth={0} aria-hidden="true" />
            <span className="sr-only">{t("common.stop")}</span>
          </button>
        ) : hasInput ? (
          <button type="submit" className="composer-send" disabled={busy}>
            <ArrowUp strokeWidth={2.2} aria-hidden="true" />
            <span className="sr-only">{queueing ? t("thread.composer.queue") : t("common.send")}</span>
          </button>
        ) : (
          <MicButton dictation={dictation} variant={home ? "glyph" : "circle"} />
        )}
      </div>
    </div>
  );

  return (
    <div className="composer-dock" data-variant={variant}>
      {sessionId && !home ? <OutboxNotice sessionId={sessionId} /> : null}
      {error ? (
        <p role="alert" className="composer-error">
          {error}
        </p>
      ) : null}
      <form ref={form} className="composer" data-variant={home ? "home" : "thread"} onSubmit={send}>
        <div className="composer-card">
          {chips.length || files.length || attached.length || unattached.length ? (
            <ul className="composer-chips" aria-label={t("thread.composer.chips")}>
              {chips.map((name) => (
                <li key={`s-${name}`} className="chip skill-chip">
                  /{name}
                  <button type="button" className="chip-remove" aria-label={t("thread.composer.removeSkill", { name })} onClick={() => removeMention(name)}>
                    <X size={14} strokeWidth={ICON_STROKE} aria-hidden="true" />
                  </button>
                </li>
              ))}
              {attached.map((tab) => {
                const title = tab.title.trim() || tab.url;
                return (
                  <li key={`t-${tab.tab_id}`} className="chip tab-chip" title={tab.url}>
                    <Globe size={12} strokeWidth={ICON_STROKE} aria-hidden="true" />
                    <span className="tab-chip-title">{title}</span>
                    <button type="button" className="chip-remove" aria-label={t("thread.composer.removeTab", { name: title })} onClick={() => detach(tab)}>
                      <X size={14} strokeWidth={ICON_STROKE} aria-hidden="true" />
                    </button>
                  </li>
                );
              })}
              {unattached.map((tab) => {
                const title = tab.title.trim() || tab.url || t("thread.context.untitledTab");
                return (
                  <li key={`u-${tab.tab_id}`} className="chip tab-chip is-failed" title={tab.url}>
                    <Globe size={12} strokeWidth={ICON_STROKE} aria-hidden="true" />
                    <span className="tab-chip-title">{title}</span>
                    <button type="button" className="chip-remove" aria-label={t("thread.composer.retryTab", { name: title })} onClick={() => void retryAttach(tab)}>
                      <RotateCcw size={12} strokeWidth={ICON_STROKE} aria-hidden="true" />
                    </button>
                    <button
                      type="button"
                      className="chip-remove"
                      aria-label={t("thread.composer.removeTab", { name: title })}
                      onClick={() => setUnattached((current) => current.filter((known) => known.tab_id !== tab.tab_id))}
                    >
                      <X size={14} strokeWidth={ICON_STROKE} aria-hidden="true" />
                    </button>
                  </li>
                );
              })}
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
                    aria-label={t("thread.composer.removeAttachment", { name: pending.file.name })}
                    onClick={() => {
                      if (locked) return;
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
            <span className="sr-only">{t("thread.composer.field")}</span>
            <textarea
              ref={textarea}
              value={text}
              onChange={(e) => update(e.target.value, e.target.selectionStart ?? e.target.value.length)}
              readOnly={locked}
              aria-busy={locked || undefined}
              onKeyDown={onKeyDown}
              onFocus={prewarm}
              rows={1}
              autoFocus={home}
              placeholder={placeholder}
              aria-autocomplete="list"
              aria-controls={contextOpen ? "context-menu" : undefined}
              aria-activedescendant={contextOpen ? `context-${contextRows[activeRow]?.id}` : undefined}
            />
          </label>
          {home ? null : bar}
        </div>
        {home ? <div className={cx("composer-below", !showBar && "is-hidden")}>{bar}</div> : null}
        {contextOpen && context ? (
          <ComposerContextMenu
            items={contextRows}
            active={activeRow}
            below={context.below}
            trigger={context.trigger}
            query={context.query}
            onPick={pickContext}
            onClose={() => setContext(null)}
          />
        ) : null}
      </form>
    </div>
  );
}
