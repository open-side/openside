import { CloudOff, RotateCw, Trash2 } from "lucide-react";
import { useClient } from "../context";
import { ICON_STROKE, IconButton } from "../ui";
import { entriesFor, outbox } from "./outbox";
import { useOutboxEntries } from "./useOutbox";

/**
 * Messages typed while the daemon was unreachable wait here until they are delivered. Shown
 * inside a conversation's composer dock (or the new-chat screen's, for `sessionId === null`).
 */
export function OutboxNotice({ sessionId }: { sessionId: string | null }) {
  const client = useClient();
  const mine = entriesFor(useOutboxEntries(), sessionId);
  if (mine.length === 0) return null;
  const failed = mine.find((entry) => entry.failure);
  const waiting = mine.filter((entry) => !entry.failure).length;
  return (
    <div className="outbox-notice" role="status" data-failed={failed ? "" : undefined}>
      <CloudOff size={16} strokeWidth={ICON_STROKE} aria-hidden="true" />
      <span className="outbox-notice-text">
        {failed
          ? `Couldn’t send: ${failed.failure}`
          : waiting === 1
            ? "1 message waiting to send"
            : `${waiting} messages waiting to send`}
      </span>
      <span className="outbox-notice-actions">
        {failed ? null : (
          <IconButton label="Send now" size="small" onClick={() => void outbox.flush(client)}>
            <RotateCw size={16} strokeWidth={ICON_STROKE} aria-hidden="true" />
          </IconButton>
        )}
        <IconButton label={failed ? "Discard message" : "Discard queued messages"} size="small" onClick={() => mine.forEach((entry) => void outbox.discard(entry.message_id))}>
          <Trash2 size={16} strokeWidth={ICON_STROKE} aria-hidden="true" />
        </IconButton>
      </span>
    </div>
  );
}
