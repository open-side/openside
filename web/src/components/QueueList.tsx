import { CornerDownRight, Ellipsis, ListEnd, Pencil, Send, X } from "lucide-react";
import { useState } from "react";
import { ApiError } from "../api/client";
import { useClient } from "../context";
import type { QueuedMessage } from "../state/session";
import { Button, ICON_STROKE, IconButton, MenuItem, PopoverMenu } from "../ui";

const icon = { size: 18, strokeWidth: ICON_STROKE, "aria-hidden": true } as const;

function QueueRow({ sessionId, message, running }: { sessionId: string; message: QueuedMessage; running: boolean }) {
  const client = useClient();
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState(message.text);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function act(action: () => Promise<unknown>, after?: () => void) {
    setBusy(true);
    setError(null);
    try {
      await action();
      after?.();
    } catch (cause) {
      setError(cause instanceof ApiError ? cause.message : "Request failed");
    } finally {
      setBusy(false);
    }
  }

  const steering = message.mode === "steer";
  return (
    <li className="queue-item" data-message={message.id} data-mode={message.mode}>
      <div className="queue-meta">
        <ListEnd size={15} strokeWidth={ICON_STROKE} aria-hidden="true" />
        <span>{editing ? "Editing" : steering ? "Steering" : "Queued"}</span>
        {!editing && (
          <PopoverMenu
            label="Queued message options"
            trigger={
              <IconButton label="Message options" size="small" disabled={busy}>
                <Ellipsis {...icon} />
              </IconButton>
            }
          >
            <MenuItem
              icon={<Pencil {...icon} />}
              onClick={() => {
                setDraft(message.text);
                setEditing(true);
              }}
            >
              Edit message
            </MenuItem>
            {!steering && (
              <MenuItem
                icon={running ? <CornerDownRight {...icon} /> : <Send {...icon} />}
                onClick={() => void act(() => client.promoteQueued(sessionId, message.id))}
              >
                {running ? "Steer instead" : "Send now"}
              </MenuItem>
            )}
            <MenuItem icon={<X {...icon} />} danger onClick={() => void act(() => client.deleteQueued(sessionId, message.id))}>
              Cancel message
            </MenuItem>
          </PopoverMenu>
        )}
      </div>
      {editing ? (
        <form
          className="queue-edit"
          onSubmit={(event) => {
            event.preventDefault();
            if (!draft.trim()) return;
            void act(() => client.editQueued(sessionId, message.id, draft), () => setEditing(false));
          }}
        >
          <label>
            <span className="sr-only">Edit queued message</span>
            <textarea value={draft} onChange={(e) => setDraft(e.target.value)} rows={3} disabled={busy} />
          </label>
          <div className="queue-edit-actions">
            <Button size="compact" disabled={busy} onClick={() => setEditing(false)}>
              Cancel
            </Button>
            <Button type="submit" size="compact" variant="primary" disabled={busy || !draft.trim()}>
              Save
            </Button>
          </div>
        </form>
      ) : (
        <p className="queue-bubble">{message.text}</p>
      )}
      {error ? (
        <p role="alert" className="queue-error">
          {error}
        </p>
      ) : null}
    </li>
  );
}

/**
 * Pending follow-ups as right-aligned bubbles under the transcript, steering lane first.
 * `running` decides whether promoting a follow-up reads "Steer instead" or "Send now".
 */
export function QueueList({ sessionId, queue, running = false }: { sessionId: string; queue: QueuedMessage[]; running?: boolean }) {
  if (!queue.length) return null;
  return (
    <section className="queue" aria-label="Queued messages">
      <ol>
        {queue.map((message) => (
          <QueueRow key={message.id} sessionId={sessionId} message={message} running={running} />
        ))}
      </ol>
    </section>
  );
}
