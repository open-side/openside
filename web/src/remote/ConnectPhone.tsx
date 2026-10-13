import { Check, ChevronRight, Copy, QrCode as QrIcon, RefreshCw, TriangleAlert } from "lucide-react";
import { useEffect, useId, useRef, useState } from "react";
import { Link, useLocation } from "react-router";
import type { CapabilityScope, Device, PairingOffer, RemoteStatus } from "../api/types";
import { useClient } from "../context";
import { errorMessage } from "../hooks/useResource";
import { manageLinkState } from "../pages/manageRoutes";
import { ErrorNote } from "../pages/blocks";
import { Button, copyText, ICON_STROKE, QrCode, Segmented, Spinner, useToast } from "../ui";
import { LocalOnlyNotice } from "./LocalOnlyNotice";
import { connectionProblem, enableTransport, oneClickTransport, pairingOptions, TRANSPORT_NAMES, type ConnectionProblem, type PairingOption, type TransportId } from "./transports";
import "./remote.css";

const POLL_MS = 3_000;
const TICK_MS = 1_000;

interface ScopeChoice {
  id: CapabilityScope;
  label: string;
  /** Always granted: a phone that cannot read is not connected to anything. */
  locked?: boolean;
}

const SCOPE_CHOICES: ScopeChoice[] = [
  { id: "read", label: "See conversations and results", locked: true },
  { id: "steer", label: "Start tasks and send messages" },
  { id: "approve", label: "Approve or reject what the agent asks" },
  { id: "human_step", label: "Do steps that need a person, such as logins" },
  { id: "browse", label: "Watch and use the agent’s browser" },
  { id: "admin", label: "Change settings and manage other devices" },
];

const DEFAULT_NAME = "My phone";
const DEFAULT_SCOPES: CapabilityScope[] = ["read", "steer", "approve", "human_step", "browse"];

interface Request {
  /** Bumped by every explicit "new code" so an unchanged connection still issues another offer. */
  n: number;
  name: string;
  scopes: CapabilityScope[];
}

/** An issued offer and the request and connection it was issued for. */
interface Issued {
  key: string;
  offer: PairingOffer;
}

function countdown(seconds: number): string {
  return `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, "0")}`;
}

/** The QR tile keeps its size in every state so the screen does not jump while a code is made. */
function QrTile({ children, state }: { children: React.ReactNode; state: "ready" | "empty" | "expired" }) {
  return (
    <div className="remote-qr" data-state={state}>
      {children}
    </div>
  );
}

/** Nothing can carry a code yet: say why in place of the QR code, and offer the one obvious fix. */
function Unavailable({
  problem,
  status,
  onStatusStale,
  chosen,
}: {
  problem: ConnectionProblem;
  status: RemoteStatus;
  onStatusStale: () => void;
  chosen: TransportId | null;
}) {
  const client = useClient();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const target = problem.kind === "none" ? (chosen ?? oneClickTransport(status)) : problem.id;
  const canChange = status.configure_allowed;

  async function turnOn() {
    setBusy(true);
    setError(null);
    try {
      await enableTransport(client, target, status);
      onStatusStale();
    } catch (cause) {
      setError(errorMessage(cause));
    } finally {
      setBusy(false);
    }
  }

  const name = TRANSPORT_NAMES[target];
  return (
    <div className="pair-frame">
    <div className="pair" data-state={problem.kind === "starting" ? "starting" : "down"}>
      <QrTile state="empty">
        <div className="remote-qr-empty" aria-hidden="true">
          {problem.kind === "starting" ? <Spinner size={28} /> : problem.kind === "error" ? <TriangleAlert size={36} strokeWidth={ICON_STROKE} /> : <QrIcon size={44} strokeWidth={ICON_STROKE} />}
        </div>
      </QrTile>
      <div className="pair-body">
      {problem.kind === "none" ? (
        <>
          <p className="pair-status">
            <span>Your phone needs a way to reach this computer.</span>
          </p>
          {target === "cloudflare" && <p className="pair-hint">A Cloudflare quick tunnel needs no account. It opens a temporary public address that only a QR code from here points to.</p>}
        </>
      ) : problem.kind === "starting" ? (
        <p className="pair-status" role="status">
          <span>{problem.message}</span>
        </p>
      ) : (
        <p className="pair-status pair-status--error" role="alert">
          <span>{problem.message}</span>
        </p>
      )}
      {error && <ErrorNote>{error}</ErrorNote>}
      {canChange && problem.kind !== "starting" && (
        <div className="pair-actions">
          <Button variant="primary" disabled={busy} onClick={() => void turnOn()}>
            {busy && <Spinner size={16} />}
            {problem.kind === "error" ? "Try again" : `Turn on ${name}`}
          </Button>
        </div>
      )}
      </div>
    </div>
    </div>
  );
}

/**
 * Pairs a phone with one glance: opening it already shows the QR code, made for the best
 * connection that is on (the daemon's preferred one). Waiting, connected and expired are one
 * status line under the code; changing the connection, the phone's name and its permissions sit
 * below as secondary controls. Used by Settings → Remote access and the setup guide.
 */
export function ConnectPhone({
  status,
  onStatusStale,
  onPaired,
}: {
  status: RemoteStatus;
  /** A call showed that the local-access proof is gone, or a connection changed; the caller should reload `status`. */
  onStatusStale: () => void;
  onPaired?: (device: Device) => void;
}) {
  const client = useClient();
  const toast = useToast();
  const location = useLocation();
  const nameId = useId();
  const options = pairingOptions(status);
  const [choice, setChoice] = useState<string | null>(null);
  const [request, setRequest] = useState<Request>({ n: 0, name: DEFAULT_NAME, scopes: DEFAULT_SCOPES });
  const [draftName, setDraftName] = useState(DEFAULT_NAME);
  const [draftScopes, setDraftScopes] = useState<CapabilityScope[]>(DEFAULT_SCOPES);
  const [changing, setChanging] = useState(false);
  const [customising, setCustomising] = useState(false);
  const [issued, setIssued] = useState<Issued | null>(null);
  const [failure, setFailure] = useState<{ key: string; message: string } | null>(null);
  const [paired, setPaired] = useState<Device | null>(null);
  const [now, setNow] = useState(() => Date.now());
  // Devices that existed when the code was issued; any other one is the phone.
  const known = useRef<Set<string>>(new Set());
  // The key being issued, so a status poll or a StrictMode re-run never asks for a second code.
  const inflight = useRef<string | null>(null);

  const selected: PairingOption | undefined = choice === null ? options[0] : options.find((option) => String(option.via) === choice);
  const key = selected ? `${request.n}|${selected.via}|${selected.url}` : null;
  const offer = issued && issued.key === key ? issued.offer : null;
  const failed = failure && failure.key === key ? failure.message : null;
  const remaining = offer ? Math.max(0, Math.ceil((Date.parse(offer.expires_at) - now) / 1000)) : 0;
  const expired = offer !== null && remaining === 0;
  const allowed = status.pairing_allowed;

  const selectedVia = selected?.via ?? null;
  const selectedUrl = selected?.url ?? null;
  useEffect(() => {
    if (!allowed || paired || key === null || selectedUrl === null) return;
    if (issued?.key === key || failure?.key === key || inflight.current === key) return;
    inflight.current = key;
    void (async () => {
      try {
        known.current = new Set((await client.listDevices()).devices.map((device) => device.device_id));
        const created = await client.createPairing({
          name: request.name.trim() || DEFAULT_NAME,
          scopes: SCOPE_CHOICES.filter((item) => item.locked || request.scopes.includes(item.id)).map((item) => item.id),
          kind: "person_device",
          via: selectedVia,
        });
        setNow(Date.now());
        setIssued({ key, offer: created });
      } catch (cause) {
        setFailure({ key, message: errorMessage(cause) });
        // 403 local_access_required means the proof went stale after the status was read.
        onStatusStale();
      } finally {
        inflight.current = null;
      }
    })();
  }, [allowed, paired, key, selectedVia, selectedUrl, issued, failure, request, client, onStatusStale]);

  useEffect(() => {
    if (!offer || paired) return;
    const tick = window.setInterval(() => setNow(Date.now()), TICK_MS);
    return () => window.clearInterval(tick);
  }, [offer, paired]);

  useEffect(() => {
    if (!offer || expired || paired) return;
    let cancelled = false;
    const timer = window.setInterval(async () => {
      try {
        const { devices } = await client.listDevices();
        const phone = devices.find((device) => !known.current.has(device.device_id));
        if (cancelled || !phone) return;
        setPaired(phone);
        onPaired?.(phone);
      } catch {
        // A missed poll is retried on the next tick; the code's own expiry ends the wait.
      }
    }, POLL_MS);
    return () => {
      cancelled = true;
      window.clearInterval(timer);
    };
  }, [client, offer, expired, paired, onPaired]);

  function refresh() {
    setRequest((current) => ({ ...current, n: current.n + 1 }));
  }

  function apply() {
    setRequest((current) => ({ n: current.n + 1, name: draftName, scopes: draftScopes }));
    setCustomising(false);
  }

  function another() {
    setPaired(null);
    setCustomising(false);
    refresh();
  }

  function toggle(scope: CapabilityScope, on: boolean) {
    setDraftScopes((current) => (on ? [...current, scope] : current.filter((item) => item !== scope)));
  }

  async function copy(text: string) {
    toast.notify((await copyText(text)) ? "Link copied" : "Could not copy; select the text instead", "success");
  }

  if (!allowed) return <LocalOnlyNotice what="connect a phone" onChanged={onStatusStale} />;

  if (paired) {
    return (
      <div className="remote-result" role="status">
        <span className="remote-result-mark" aria-hidden="true">
          <Check size={30} strokeWidth={ICON_STROKE} />
        </span>
        <p className="remote-result-title">Connected as {paired.name || "New device"}</p>
        <p className="remote-note">It can follow your tasks now. You can revoke it any time under Paired devices.</p>
        <div className="remote-actions">
          <Button variant="secondary" onClick={another}>
            Connect another phone
          </Button>
          <Link className="ui-button ui-button--ghost" to="/devices" replace state={manageLinkState(location)}>
            See paired devices
          </Link>
        </div>
      </div>
    );
  }

  if (!selected) {
    const chosen = choice === null || choice === "null" ? null : (choice as TransportId);
    return <Unavailable problem={connectionProblem(status, chosen)} status={status} onStatusStale={onStatusStale} chosen={chosen} />;
  }

  const payload = offer ? (offer.url ?? offer.path) : null;
  const phoneName = request.name.trim() || DEFAULT_NAME;
  const state = failed ? "failed" : expired ? "expired" : offer ? "waiting" : "creating";

  return (
    <div className="pair-frame">
    <div className="pair" data-state={state}>
      <QrTile state={expired ? "expired" : payload ? "ready" : "empty"}>
        {payload ? (
          <>
            <QrCode text={payload} label={`QR code to connect ${phoneName}`} size={264} />
            {expired && (
              <div className="remote-qr-veil">
                <span>Expired</span>
              </div>
            )}
          </>
        ) : (
          <div className="remote-qr-empty" aria-hidden="true">
            {failed ? <TriangleAlert size={36} strokeWidth={ICON_STROKE} /> : <Spinner size={28} />}
          </div>
        )}
      </QrTile>

      <div className="pair-body">
      {failed ? (
        <>
          <ErrorNote>{failed}</ErrorNote>
          <div className="pair-actions">
            <Button variant="primary" onClick={refresh}>
              <RefreshCw size={16} strokeWidth={ICON_STROKE} aria-hidden="true" />
              Try again
            </Button>
          </div>
        </>
      ) : expired ? (
        <>
          <p className="pair-status">
            <span>This code expired.</span>
          </p>
          <div className="pair-actions">
            <Button variant="primary" onClick={refresh}>
              <RefreshCw size={16} strokeWidth={ICON_STROKE} aria-hidden="true" />
              Make a new code
            </Button>
          </div>
        </>
      ) : (
        <>
          <p className="pair-status" role="status">
            <Spinner size={16} />
            <span>{offer ? "Waiting for your phone…" : "Getting your code…"}</span>
          </p>
          {payload && (
            <>
              <p className="pair-hint">Point your phone’s camera at the code, then tap the link that appears.</p>
              <div className="remote-url pair-link">
                <code title={payload}>{payload}</code>
                <Button size="compact" variant="ghost" aria-label="Copy the pairing link" onClick={() => void copy(payload)}>
                  <Copy size={16} strokeWidth={ICON_STROKE} aria-hidden="true" />
                  Copy
                </Button>
              </div>
              <p className="pair-expiry">
                <span>
                  Expires in{" "}
                  <span role="timer" aria-live="off">
                    {countdown(remaining)}
                  </span>
                </span>
                <Button size="compact" variant="ghost" onClick={refresh}>
                  <RefreshCw size={14} strokeWidth={ICON_STROKE} aria-hidden="true" />
                  New code
                </Button>
              </p>
            </>
          )}
        </>
      )}
      </div>

      <div className="pair-secondary">
        <p className="pair-via">
          <span>
            Through <strong>{selected.label}</strong>
          </span>
          {options.length > 1 && (
            <Button size="compact" variant="ghost" aria-expanded={changing} onClick={() => setChanging((open) => !open)}>
              Change connection
            </Button>
          )}
        </p>
        {changing && options.length > 1 && (
          <Segmented
            label="Connect through"
            options={options.map((option) => ({ value: String(option.via), label: option.label }))}
            value={String(selected.via)}
            onChange={(value) => {
              setChoice(value);
              setChanging(false);
            }}
          />
        )}
        <button type="button" className="row-disclosure pair-options-toggle" aria-expanded={customising} onClick={() => setCustomising((open) => !open)}>
          <ChevronRight size={16} strokeWidth={ICON_STROKE} aria-hidden="true" />
          Phone name and permissions
        </button>
        {customising && (
          <form
            className="remote-form"
            onSubmit={(event) => {
              event.preventDefault();
              apply();
            }}
          >
            <div className="remote-field">
              <label htmlFor={nameId}>Phone name</label>
              <input id={nameId} value={draftName} maxLength={60} onChange={(event) => setDraftName(event.target.value)} autoComplete="off" />
            </div>
            <fieldset className="remote-scopes">
              <legend>What the phone may do</legend>
              {SCOPE_CHOICES.map((item) => (
                <label key={item.id} className="remote-scope" data-risk={item.id === "admin" ? "high" : undefined}>
                  <input
                    type="checkbox"
                    checked={item.locked || draftScopes.includes(item.id)}
                    disabled={item.locked}
                    onChange={(event) => toggle(item.id, event.target.checked)}
                  />
                  <span>{item.label}</span>
                </label>
              ))}
            </fieldset>
            <div className="remote-actions">
              <Button variant="primary" type="submit">
                Make a new code
              </Button>
            </div>
          </form>
        )}
      </div>
    </div>
    </div>
  );
}
