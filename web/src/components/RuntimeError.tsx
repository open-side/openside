import { CircleAlert, RotateCw } from "lucide-react";
import { useState } from "react";
import type { Attachment } from "../api/types";
import { useClient } from "../context";
import type { TimelineItem } from "../state/session";
import { retryMessage } from "../state/retry";
import { Button, ICON_STROKE } from "../ui";

/** Plain first-person lines for the codes the daemon names, for events recorded before it wrote one. */
const COPY: Record<string, string> = {
  interrupted: "I was interrupted before I finished.",
  sandbox_unavailable: "I couldn’t start the sandbox, so shell commands run without one.",
  browser_tab_limit: "Too many tabs are open, so I stopped.",
};

const FALLBACK = "Something went wrong on my side.";

export interface RuntimeErrorItem {
  code: string;
  message: string;
  userMessage?: string | undefined;
  retryable?: boolean | undefined;
}

/** The message a failure at `items[at]` can be retried with: the person's last words, unless they have written since. */
export function retrySource(items: readonly TimelineItem[], at: number): { text: string; attachments?: Attachment[] } | undefined {
  const theirs = (item: TimelineItem) => item.type === "user" && item.author_kind === "person";
  if (items.slice(at + 1).some(theirs)) return undefined;
  for (let index = at - 1; index >= 0; index -= 1) {
    const item = items[index]!;
    if (item.type === "user" && theirs(item)) return { text: item.text, ...(item.attachments ? { attachments: item.attachments } : {}) };
  }
  return undefined;
}

/**
 * A failed turn: one calm line in the agent's voice and the obvious next step. The daemon's own
 * wording and the code stay behind "Technical details".
 */
export function RuntimeErrorRow({
  error,
  sessionId,
  retry,
}: {
  error: RuntimeErrorItem;
  sessionId: string;
  /** The person's last message before the failure; absent when it was answered since or never existed. */
  retry?: { text: string; attachments?: Attachment[] };
}) {
  const client = useClient();
  const [busy, setBusy] = useState(false);
  const [failure, setFailure] = useState<string | null>(null);
  const retryable = error.retryable ?? error.code === "interrupted";
  const line = error.userMessage?.trim() || COPY[error.code] || FALLBACK;

  async function again() {
    if (!retry) return;
    setBusy(true);
    setFailure(null);
    const result = await retryMessage(client, sessionId, retry);
    setBusy(false);
    if (result) setFailure(result.status === 0 ? "Can’t reach openside" : result.message);
  }

  return (
    <div className="runtime-error" role="status">
      <CircleAlert size={18} strokeWidth={ICON_STROKE} aria-hidden="true" />
      <div className="runtime-error-body">
        <p className="runtime-error-title">{line}</p>
        {retryable && retry ? (
          <Button size="compact" disabled={busy} onClick={() => void again()}>
            <RotateCw size={14} strokeWidth={ICON_STROKE} aria-hidden="true" />
            {busy ? "Retrying…" : "Retry"}
          </Button>
        ) : null}
        {failure ? (
          <p role="alert" className="runtime-error-failure">
            {failure}
          </p>
        ) : null}
        <details className="runtime-error-tech">
          <summary>Technical details</summary>
          {error.message ? <p>{error.message}</p> : null}
          <p className="runtime-error-code">{error.code}</p>
        </details>
      </div>
    </div>
  );
}
