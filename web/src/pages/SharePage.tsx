import { CircleAlert, Download } from "lucide-react";
import { useEffect, useMemo, useState } from "react";
import { ApiError, createClient, type Client } from "../api/client";
import type { SharedArtifact, SharedChat, SharedItem } from "../api/types";
import { Markdown } from "../components/Markdown";
import { describeTool } from "../components/toolCopy";
import { displaySessionTitle } from "../components/toolStatus";
import { t, type MessageKey } from "../i18n";
import { BrandMark, Button, ICON_STROKE, Spinner } from "../ui";
import { formatBytes, formatDateTime } from "./format";
import "./share.css";

type Load =
  | { state: "loading" }
  | { state: "ready"; chat: SharedChat }
  | { state: "failed"; title: MessageKey; detail: string };

// Matches the daemon's share ids: 32 random bytes, base64url without padding.
const SHARE_ID = /^[A-Za-z0-9_-]{43}$/;
const INLINE_IMAGE: Record<string, true> = { "image/png": true, "image/jpeg": true, "image/gif": true, "image/webp": true };
const icon = { size: 16, strokeWidth: ICON_STROKE, "aria-hidden": true } as const;

function failure(cause: unknown): Load {
  if (cause instanceof ApiError && cause.status === 404) return { state: "failed", title: "share.page.goneTitle", detail: t("share.page.gone") };
  if (cause instanceof ApiError && cause.status === 429) return { state: "failed", title: "share.page.busyTitle", detail: t("share.page.busy") };
  return { state: "failed", title: "share.page.failedTitle", detail: cause instanceof ApiError ? cause.message : t("api.error.network_error") };
}

/**
 * `/share#<id>`: a read-only snapshot someone published from their chat. It runs without a
 * device token; the id in the fragment is the credential, and the fragment never leaves the
 * browser in a Referer header.
 */
export function SharePage() {
  const [id] = useState(() => {
    try {
      return decodeURIComponent(window.location.hash.slice(1)).trim();
    } catch {
      // A malformed escape is just an incomplete link.
      return "";
    }
  });
  const client = useMemo(() => createClient({ token: () => null }), []);
  const [load, setLoad] = useState<Load>(() =>
    SHARE_ID.test(id) ? { state: "loading" } : { state: "failed", title: "share.page.missingTitle", detail: t("share.page.missing") },
  );

  useEffect(() => {
    if (!SHARE_ID.test(id)) return;
    let live = true;
    client.sharedChat(id).then(
      (chat) => {
        if (!live) return;
        document.title = displaySessionTitle(chat.title);
        setLoad({ state: "ready", chat });
      },
      (cause: unknown) => live && setLoad(failure(cause)),
    );
    return () => {
      live = false;
    };
  }, [client, id]);

  return (
    <main className="share-page">
      <header className="share-bar">
        <BrandMark size={20} />
        <span>openside</span>
      </header>
      {load.state === "loading" ? (
        <p role="status" className="share-status">
          <Spinner size={16} />
          <span>{t("share.page.loading")}</span>
        </p>
      ) : load.state === "failed" ? (
        <section className="share-problem" role="alert">
          <CircleAlert size={20} strokeWidth={ICON_STROKE} aria-hidden="true" />
          <h1>{t(load.title)}</h1>
          <p>{load.detail}</p>
        </section>
      ) : (
        <SharedConversation client={client} id={id} chat={load.chat} />
      )}
    </main>
  );
}

function SharedConversation({ client, id, chat }: { client: Client; id: string; chat: SharedChat }) {
  return (
    <article className="share-chat">
      <header className="share-head">
        <p className="share-eyebrow">{t("share.page.label")}</p>
        <h1>{displaySessionTitle(chat.title)}</h1>
        <p className="share-meta">
          {t("share.page.snapshot", { date: formatDateTime(chat.shared_at) })}
          {chat.expires_at ? <> {t("share.page.expires", { date: formatDateTime(chat.expires_at) })}</> : null}
        </p>
      </header>
      {chat.items.length === 0 ? <p className="share-meta">{t("share.page.empty")}</p> : null}
      <ol className="share-items">
        {chat.items.map((item, index) => (
          <li key={index}>
            <SharedEntry item={item} />
          </li>
        ))}
      </ol>
      {chat.artifacts.length ? (
        <section className="share-files" aria-labelledby="share-files">
          <h2 id="share-files">{t("share.page.files")}</h2>
          <ul>
            {chat.artifacts.map((artifact) => (
              <SharedFile key={artifact.index} client={client} id={id} artifact={artifact} />
            ))}
          </ul>
        </section>
      ) : null}
    </article>
  );
}

function SharedEntry({ item }: { item: SharedItem }) {
  if (item.kind === "user") return <p className="share-user">{item.text}</p>;
  if (item.kind === "assistant") {
    return (
      <div className="share-assistant">
        <Markdown text={item.text} />
      </div>
    );
  }
  return (
    <details className="share-steps">
      <summary>{t("share.page.steps", { count: item.steps.length })}</summary>
      <ul>
        {item.steps.map((step, index) => {
          // Only the tool name is shared, so the step reads as its generic past-tense phrase.
          const { title, Icon } = describeTool({ type: "tool", id: String(index), name: step.tool, args: {}, updates: [], done: true });
          return (
            <li key={index} data-failed={step.failed || undefined}>
              <Icon {...icon} />
              <span>{title}</span>
              {step.failed ? <span className="share-step-failed">{t("share.page.stepFailed")}</span> : null}
            </li>
          );
        })}
      </ul>
    </details>
  );
}

function SharedFile({ client, id, artifact }: { client: Client; id: string; artifact: SharedArtifact }) {
  const image = INLINE_IMAGE[artifact.mime] === true;
  const [preview, setPreview] = useState<string | null>(null);

  useEffect(() => {
    if (!image) return;
    let url: string | null = null;
    let live = true;
    client.sharedArtifactBlob(id, artifact.index).then(
      (blob) => {
        if (!live) return;
        url = URL.createObjectURL(blob);
        setPreview(url);
      },
      () => undefined,
    );
    return () => {
      live = false;
      if (url) URL.revokeObjectURL(url);
    };
  }, [client, id, artifact.index, image]);

  async function download() {
    const blob = await client.sharedArtifactBlob(id, artifact.index);
    const url = URL.createObjectURL(blob);
    const anchor = document.createElement("a");
    anchor.href = url;
    anchor.download = artifact.name;
    anchor.click();
    setTimeout(() => URL.revokeObjectURL(url), 0);
  }

  return (
    <li className="share-file">
      {preview ? <img src={preview} alt={artifact.name} /> : null}
      <span className="share-file-name">{artifact.name}</span>
      <span className="share-file-size">{formatBytes(artifact.size)}</span>
      <Button size="small" onClick={() => void download().catch(() => undefined)}>
        <Download {...icon} />
        {t("share.page.download")}
      </Button>
    </li>
  );
}
