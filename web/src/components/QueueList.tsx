import { CornerDownRight, Ellipsis, ListEnd, Pencil, Send, X } from "lucide-react";
import { useState } from "react";
import { ApiError } from "../api/client";
import { useClient } from "../context";
import { t } from "../i18n";
import type { QueuedMessage } from "../state/session";
import { Button, ICON_STROKE, IconButton, MenuItem, PopoverMenu } from "../ui";
import { MessageMenu } from "./MessageMenu";

const icon = { size: 18, strokeWidth: ICON_STROKE, "aria-hidden": true } as const;

function QueueRow({
  sessionId,
  message,
  running,
  onQuote,
}: {
  sessionId: string;
  message: QueuedMessage;
  running: boolean;
  onQuote?: ((text: string) => void) | undefined;
}) {
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
      setError(cause instanceof ApiError ? cause.message : t("thread.requestFailed"));
    } finally {
      setBusy(false);
    }
  }

  const steering = message.mode === "steer";
  const edit = (
    <MenuItem
      icon={<Pencil {...icon} />}
      onClick={() => {
        setDraft(message.text);
        setEditing(true);
      }}
    >
      {t("thread.queue.editMessage")}
    </MenuItem>
  );
  return (
    <li className="queue-item" data-message={message.id} data-mode={message.mode}>
      <div className="queue-meta">
        <ListEnd size={15} strokeWidth={ICON_STROKE} aria-hidden="true" />
        <span>{editing ? t("thread.queue.editing") : steering ? t("thread.queue.steering") : t("thread.queue.queued")}</span>
        {!editing && (
          <PopoverMenu
            label={t("thread.queue.optionsMenu")}
            trigger={
              <IconButton label={t("thread.queue.options")} size="small" disabled={busy}>
                <Ellipsis {...icon} />
              </IconButton>
            }
          >
            {edit}
            {!steering && (
              <MenuItem
                icon={running ? <CornerDownRight {...icon} /> : <Send {...icon} />}
                onClick={() => void act(() => client.promoteQueued(sessionId, message.id))}
              >
                {running ? t("thread.queue.steerInstead") : t("thread.queue.sendNow")}
              </MenuItem>
            )}
            <MenuItem icon={<X {...icon} />} danger onClick={() => void act(() => client.deleteQueued(sessionId, message.id))}>
              {t("thread.queue.cancelMessage")}
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
            <span className="sr-only">{t("thread.queue.editField")}</span>
            <textarea value={draft} onChange={(e) => setDraft(e.target.value)} rows={3} disabled={busy} />
          </label>
          <div className="queue-edit-actions">
            <Button disabled={busy} onClick={() => setEditing(false)}>
              {t("common.cancel")}
            </Button>
            <Button type="submit" variant="primary" disabled={busy || !draft.trim()}>
              {t("common.save")}
            </Button>
          </div>
        </form>
      ) : (
        <MessageMenu element="p" className="queue-bubble" text={message.text} onQuote={onQuote} extra={busy ? null : edit}>
          {message.text}
        </MessageMenu>
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
export function QueueList({
  sessionId,
  queue,
  running = false,
  onQuote,
}: {
  sessionId: string;
  queue: QueuedMessage[];
  running?: boolean;
  /** Puts a quoted message into the composer. */
  onQuote?: ((text: string) => void) | undefined;
}) {
  const mine = queue.filter((message) => message.author_kind !== "internal");
  if (!mine.length) return null;
  return (
    <section className="queue" aria-label={t("thread.queue.list")}>
      <ol>
        {mine.map((message) => (
          <QueueRow key={message.id} sessionId={sessionId} message={message} running={running} onQuote={onQuote} />
        ))}
      </ol>
    </section>
  );
}
