import { Check, CircleAlert, Hand, KeyRound, ListChecks, LogIn, MessageCircleQuestion, MonitorSmartphone, ShieldCheck, type LucideIcon } from "lucide-react";
import { useRef, useState, type FormEvent } from "react";
import { ApiError } from "../api/client";
import type { Ticket } from "../api/types";
import { authOutcome, isApprovalTicket, isChoiceTicket, isLoginTicket } from "../otp/loginForm";
import { isOtpTicket, otpRegistrationOf, registrationNote, registrationOutcome, registrationQuestion } from "../otp/otpForm";
import { KIND_LABEL, siteNote, TAKEOVER_NOTE } from "../otp/ticketCopy";
import { useClient } from "../context";
import { haptic, type Haptic } from "../pwa/haptics";
import { Button, ICON_STROKE } from "../ui";
import { AuthForm } from "./AuthForm";
import { TakeoverView } from "./TakeoverView";

const KIND_ICON: Record<Ticket["kind"], LucideIcon> = {
  confirm: ShieldCheck,
  info: MessageCircleQuestion,
  secret_input: KeyRound,
  device_approval: MonitorSmartphone,
  takeover: Hand,
  login: LogIn,
  choice: ListChecks,
};

function outcome(ticket: Ticket): string {
  const resolution = (ticket.resolution ?? {}) as Record<string, unknown>;
  switch (ticket.status) {
    case "resolved":
      if (ticket.kind === "confirm") return registrationOutcome(ticket.resolution) ?? (resolution.approved ? "Approved" : "Denied");
      if (ticket.kind === "info" && typeof resolution.answer === "string") return `Answered: ${resolution.answer}`;
      if (ticket.kind === "secret_input" && resolution.registered) return "Authenticator saved";
      if (ticket.kind === "secret_input") return ticket.otp ? "Code submitted" : "Secret submitted";
      return "Done";
    case "rejected":
      return "Rejected";
    case "expired":
      return "Expired";
    case "needs_desktop":
      return "Needs the desktop";
    default:
      return "Waiting";
  }
}

export function TicketCard({
  ticket,
  subagentTask,
  focus,
  onResolved,
}: {
  ticket: Ticket;
  subagentTask?: string;
  /** Passed to the code form; the phone sheet forces focus. */
  focus?: "polite" | "force";
  /** After this card resolved or rejected its ticket (the dock and the phone sheet close then). */
  onResolved?: () => void;
}) {
  const client = useClient();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [answer, setAnswer] = useState("");
  // Uncontrolled on purpose: the secret never enters React state or props, and the
  // field is emptied before the request is even awaited.
  const secretRef = useRef<HTMLInputElement>(null);
  const pending = ticket.status === "pending";
  const [controlling, setControlling] = useState(false);
  const remote = ticket.kind === "takeover" && !!ticket.target?.tab_id;

  async function act(action: () => Promise<unknown>, feel: Haptic = "approve") {
    setBusy(true);
    setError(null);
    try {
      await action();
      haptic(feel);
      onResolved?.();
    } catch (cause) {
      setError(cause instanceof ApiError ? cause.message : "Request failed");
    } finally {
      setBusy(false);
    }
  }

  function submitSecret(event: FormEvent) {
    event.preventDefault();
    const input = secretRef.current;
    if (!input || !input.value) return;
    const value = input.value;
    input.value = "";
    void act(() => client.resolveTicket(ticket.id, value));
  }

  function submitAnswer(event: FormEvent) {
    event.preventDefault();
    const text = answer.trim();
    if (!text) return;
    void act(async () => {
      await client.resolveTicket(ticket.id, text);
      setAnswer("");
    });
  }

  const reject = () => act(() => client.rejectTicket(ticket.id), "reject");
  const registration = otpRegistrationOf(ticket);
  const KindIcon = registration ? KeyRound : KIND_ICON[ticket.kind];
  const kindLabel = ticket.otp ? "Verification code" : registration ? "Save authenticator" : KIND_LABEL[ticket.kind];
  const origin = ticket.origin.subagent_id ? (
    <span className="chip ticket-origin" data-subagent-id={ticket.origin.subagent_id} title={ticket.origin.subagent_id}>
      {subagentTask ?? `Subagent ${ticket.origin.subagent_id.slice(0, 8)}`}
    </span>
  ) : null;

  // The code, login, choice and phone-approval cards carry their own heading (site icon, title,
  // close), so the ticket adds only who asked.
  if (pending && (isOtpTicket(ticket) || isLoginTicket(ticket) || isChoiceTicket(ticket) || isApprovalTicket(ticket))) {
    return (
      <section className="ticket ticket-pending ticket-otp ticket-auth" aria-label={kindLabel} data-ticket={ticket.id}>
        {origin ? <header className="ticket-head">{origin}</header> : null}
        <AuthForm ticket={ticket} focus={focus} onSubmitted={onResolved} />
      </section>
    );
  }

  // A finished login or choice stays in the conversation as one quiet line, never the values.
  const done = authOutcome(ticket);
  if (done) {
    return (
      <section className={`ticket ticket-${ticket.status} ticket-event`} aria-label={kindLabel} data-ticket={ticket.id}>
        <span className="event-chip" data-tone={done.failed ? "warn" : undefined}>
          {done.failed ? (
            <CircleAlert size={16} strokeWidth={ICON_STROKE} aria-hidden="true" />
          ) : (
            <Check size={16} strokeWidth={ICON_STROKE} aria-hidden="true" />
          )}
          {done.text}
        </span>
      </section>
    );
  }

  if (registration) {
    return (
      <section className={`ticket ticket-${ticket.status}`} aria-label={kindLabel} data-ticket={ticket.id}>
        <header className="ticket-head">
          <KindIcon className="ticket-icon" size={18} strokeWidth={ICON_STROKE} aria-hidden="true" />
          <span className="ticket-kind">{kindLabel}</span>
          {origin}
          {!pending ? <span className="chip">{outcome(ticket)}</span> : null}
        </header>
        <p className="ticket-summary">{registrationQuestion(registration)}</p>
        {pending ? (
          <>
            <p className="ticket-note">{registrationNote(registration)}</p>
            <div className="actions">
              <Button type="button" variant="primary" disabled={busy} onClick={() => act(() => client.resolveTicket(ticket.id, true))}>
                Save to vault
              </Button>
              <Button type="button" disabled={busy} onClick={() => act(() => client.resolveTicket(ticket.id, false), "reject")}>
                Not now
              </Button>
            </div>
          </>
        ) : null}
        {error ? (
          <p role="alert" className="ticket-error">
            {error}
          </p>
        ) : null}
      </section>
    );
  }

  return (
    <section className={`ticket ticket-${ticket.status}`} aria-label={kindLabel} data-ticket={ticket.id}>
      <header className="ticket-head">
        <KindIcon className="ticket-icon" size={18} strokeWidth={ICON_STROKE} aria-hidden="true" />
        <span className="ticket-kind">{kindLabel}</span>
        {origin}
        {!pending ? <span className="chip">{outcome(ticket)}</span> : null}
      </header>
      <p className="ticket-summary">{ticket.summary}</p>
      {ticket.instructions ? <p className="ticket-instructions">{ticket.instructions}</p> : null}
      {ticket.tool ? (
        <details className="ticket-tool">
          <summary>
            Details <code>{ticket.tool.name}</code>
          </summary>
          <pre>{JSON.stringify(ticket.tool.args, null, 2)}</pre>
        </details>
      ) : null}
      {ticket.target?.origin ? <p className="ticket-note">{siteNote(ticket.target.origin)}</p> : null}

      {pending && ticket.kind === "confirm" ? (
        <div className="actions">
          <Button type="button" variant="primary" disabled={busy} onClick={() => act(() => client.resolveTicket(ticket.id, true))}>
            Approve
          </Button>
          <Button type="button" disabled={busy} onClick={() => act(() => client.resolveTicket(ticket.id, false), "reject")}>
            Reject
          </Button>
        </div>
      ) : null}

      {pending && ticket.kind === "info" ? (
        ticket.options.length ? (
          <div className="actions" role="group" aria-label="Answers">
            {ticket.options.map((option) => (
              <Button
                key={option.value}
                type="button"
                disabled={busy}
                onClick={() => act(() => client.resolveTicket(ticket.id, option.value))}
              >
                {option.label}
              </Button>
            ))}
            <Button type="button" variant="ghost" disabled={busy} onClick={reject}>
              Dismiss
            </Button>
          </div>
        ) : (
          <form className="actions" onSubmit={submitAnswer}>
            <label className="grow">
              <span className="sr-only">Answer</span>
              <input value={answer} onChange={(e) => setAnswer(e.target.value)} placeholder="Answer" disabled={busy} />
            </label>
            <Button type="submit" variant="primary" disabled={busy || !answer.trim()}>
              Send
            </Button>
            <Button type="button" variant="ghost" disabled={busy} onClick={reject}>
              Dismiss
            </Button>
          </form>
        )
      ) : null}

      {pending && ticket.kind === "secret_input" ? (
        <form className="actions" onSubmit={submitSecret}>
          <label className="grow">
            <span className="sr-only">Secret value</span>
            <input
              ref={secretRef}
              type="password"
              name="openside-secret"
              autoComplete="off"
              autoCapitalize="off"
              spellCheck={false}
              placeholder="Secret value"
              disabled={busy}
            />
          </label>
          <Button type="submit" variant="primary" disabled={busy}>
            Submit
          </Button>
          <Button type="button" variant="ghost" disabled={busy} onClick={reject}>
            Reject
          </Button>
        </form>
      ) : null}

      {pending && (ticket.kind === "device_approval" || ticket.kind === "takeover") ? (
        <div className="actions">
          {ticket.kind === "takeover" ? (
            remote ? (
              <Button type="button" disabled={busy} onClick={() => setControlling(true)}>
                Open remote screen
              </Button>
            ) : (
              <p className="ticket-note">{TAKEOVER_NOTE}</p>
            )
          ) : null}
          <Button type="button" variant="primary" disabled={busy} onClick={() => act(() => client.ticketDone(ticket.id))}>
            Done
          </Button>
          <Button type="button" variant="ghost" disabled={busy} onClick={reject}>
            Reject
          </Button>
        </div>
      ) : null}
      {controlling ? <TakeoverView ticket={ticket} onClose={() => setControlling(false)} /> : null}

      {error ? (
        <p role="alert" className="ticket-error">
          {error}
        </p>
      ) : null}
    </section>
  );
}
