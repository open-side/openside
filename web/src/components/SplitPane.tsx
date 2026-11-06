import { Columns2, Maximize2, MessagesSquare, Plus } from "lucide-react";
import { useEffect, useState } from "react";
import { useNavigate } from "react-router";
import type { Artifact } from "../api/types";
import { useClient } from "../context";
import { t } from "../i18n";
import { ThreadView } from "../pages/ThreadView";
import { useShell } from "../shell";
import { Dialog, ICON_STROKE, IconButton, useToast } from "../ui";
import { ArtifactPreview } from "./FilesTray";
import { SessionDocument } from "./MessageDocument";
import { createSessionRequest, DEFAULT_SESSION_OPTIONS } from "./sessionOptions";
import { useSplitPane } from "./splitPaneState";
import { displaySessionTitle } from "./toolStatus";
import "./splitPane.css";

const MARKDOWN = /\.(md|markdown)$/i;

/** A non-Markdown file of the session, previewed the way the Files sheet does. */
function PaneArtifact({ sessionId, path }: { sessionId: string; path: string }) {
  const client = useClient();
  const [artifact, setArtifact] = useState<Artifact | "missing" | null>(null);
  useEffect(() => {
    let current = true;
    // The listing carries the MIME type and the signed preview URL that the raw route cannot give.
    void client.listArtifacts(sessionId).then(
      (list) => current && setArtifact(list.artifacts.find((item) => item.path === path) ?? "missing"),
      () => current && setArtifact("missing"),
    );
    return () => {
      current = false;
    };
  }, [client, sessionId, path]);
  if (artifact === null) return <p className="split-status" role="status">{t("common.loading")}</p>;
  if (artifact === "missing") return <p className="doc-failed">{t("thread.split.fileFailed")}</p>;
  return <ArtifactPreview sessionId={sessionId} artifact={artifact} />;
}

function PaneFile({ sessionId, path }: { sessionId: string; path: string }) {
  return MARKDOWN.test(path) ? <SessionDocument sessionId={sessionId} path={path} /> : <PaneArtifact sessionId={sessionId} path={path} />;
}

/**
 * The thread's right-hand pane (Aside's split view): a file of this chat, a separate side chat,
 * or an empty state offering one. Renders nothing outside a `SplitPaneContext`.
 */
export function SplitPane({ sessionId }: { sessionId: string }) {
  const split = useSplitPane();
  const client = useClient();
  const shell = useShell();
  const toast = useToast();
  const navigate = useNavigate();
  const [creating, setCreating] = useState(false);
  const [expanded, setExpanded] = useState(false);
  if (!split) return null;
  const { content } = split;
  const fileTitle = content.kind === "file" ? (content.title ?? content.path.split("/").pop() ?? content.path) : null;

  const newChat = async () => {
    setCreating(true);
    try {
      const created = await client.createSession(createSessionRequest(DEFAULT_SESSION_OPTIONS));
      shell.refreshSessions();
      split.show({ kind: "chat", sessionId: created.id });
    } catch (cause) {
      toast.notify(cause instanceof Error && cause.message ? cause.message : t("thread.split.newChatFailed"), "error");
    } finally {
      setCreating(false);
    }
  };

  return (
    <aside className="split-pane" aria-label={t("thread.split.label")}>
      <div className="split-pane-head">
        <IconButton label={t("thread.split.newChat")} className="split-button" disabled={creating} onClick={() => void newChat()}>
          <Plus size={12} strokeWidth={ICON_STROKE} aria-hidden="true" />
        </IconButton>
        <span className="split-pane-title" title={content.kind === "file" ? content.path : undefined}>
          {fileTitle}
        </span>
        {content.kind === "chat" ? (
          <IconButton label={t("thread.split.openChat")} className="split-button" onClick={() => navigate(`/sessions/${encodeURIComponent(content.sessionId)}`)}>
            <Maximize2 size={12} strokeWidth={ICON_STROKE} aria-hidden="true" />
          </IconButton>
        ) : content.kind === "file" ? (
          <IconButton label={t("thread.split.openFile")} className="split-button" aria-haspopup="dialog" onClick={() => setExpanded(true)}>
            <Maximize2 size={12} strokeWidth={ICON_STROKE} aria-hidden="true" />
          </IconButton>
        ) : null}
        <IconButton label={t("thread.split.hide")} className="split-button" onClick={split.close}>
          <Columns2 size={12} strokeWidth={ICON_STROKE} aria-hidden="true" />
        </IconButton>
      </div>
      <div className="split-pane-body">
        {content.kind === "empty" ? (
          <div className="split-empty">
            <p className="split-empty-line">{t("thread.split.empty")}</p>
            <span className="split-or" aria-hidden="true">
              {t("thread.split.or")}
            </span>
            <button type="button" className="split-new" disabled={creating} onClick={() => void newChat()}>
              <MessagesSquare size={12} strokeWidth={ICON_STROKE} aria-hidden="true" />
              {t("thread.split.newChat")}
            </button>
          </div>
        ) : content.kind === "file" ? (
          <div className="split-pane-file thin-scroll">
            <PaneFile key={content.path} sessionId={sessionId} path={content.path} />
          </div>
        ) : (
          <ThreadView
            key={content.sessionId}
            sessionId={content.sessionId}
            side
            header={({ view }) => <div className="split-chat-title">{displaySessionTitle(view.title)}</div>}
          />
        )}
      </div>
      {content.kind === "file" ? (
        <Dialog open={expanded} onClose={() => setExpanded(false)} size="wide" className="tray-sheet" title={fileTitle ?? ""}>
          <div className="tray-scroll thin-scroll">
            <PaneFile key={content.path} sessionId={sessionId} path={content.path} />
          </div>
        </Dialog>
      ) : null}
    </aside>
  );
}

/** Thread-header control that shows or hides the split pane; absent where the thread cannot split. */
export function SplitToggle() {
  const split = useSplitPane();
  if (!split) return null;
  return (
    <IconButton label={split.open ? t("thread.split.hide") : t("thread.split.show")} aria-pressed={split.open} className="split-button" onClick={split.toggle}>
      <Columns2 size={12} strokeWidth={ICON_STROKE} aria-hidden="true" />
    </IconButton>
  );
}
