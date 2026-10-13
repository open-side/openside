import { Check, Copy, RefreshCw, Smartphone } from "lucide-react";
import { useEffect, useId, useRef, useState } from "react";
import { Link, useLocation } from "react-router";
import type { CapabilityScope, Device, PairingOffer, RemoteStatus } from "../api/types";
import { useClient } from "../context";
import { errorMessage } from "../hooks/useResource";
import { manageLinkState } from "../pages/manageRoutes";
import { ErrorNote } from "../pages/blocks";
import { Button, copyText, ICON_STROKE, QrCode, Segmented, Spinner, useToast } from "../ui";
import { LocalOnlyNotice } from "./LocalOnlyNotice";
import { pairingOptions } from "./transports";
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

const DEFAULT_SCOPES: CapabilityScope[] = ["read", "steer", "approve", "human_step", "browse"];

type Phase = "form" | "creating" | "waiting" | "paired";

function countdown(seconds: number): string {
  return `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, "0")}`;
}

/**
 * Pairs a phone: pick a connection and what the phone may do, show a QR code that expires, and
 * notice when the phone arrives. Used by Settings → Remote access and the setup guide.
 */
export function ConnectPhone({
  status,
  onStatusStale,
  onPaired,
}: {
  status: RemoteStatus;
  /** A call showed that the local-access proof is gone; the caller should reload `status`. */
  onStatusStale: () => void;
  onPaired?: (device: Device) => void;
}) {
  const client = useClient();
  const toast = useToast();
  const location = useLocation();
  const nameId = useId();
  const options = pairingOptions(status);
  const [name, setName] = useState("My phone");
  const [scopes, setScopes] = useState<CapabilityScope[]>(DEFAULT_SCOPES);
  const [via, setVia] = useState<string | null>(null);
  const [phase, setPhase] = useState<Phase>("form");
  const [offer, setOffer] = useState<PairingOffer | null>(null);
  const [paired, setPaired] = useState<Device | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [now, setNow] = useState(() => Date.now());
  // Devices that existed when the code was issued; any other one is the phone.
  const known = useRef<Set<string>>(new Set());

  const selected = options.find((option) => String(option.via) === via) ?? options[0];
  const remaining = offer ? Math.max(0, Math.ceil((Date.parse(offer.expires_at) - now) / 1000)) : 0;
  const expired = phase === "waiting" && remaining === 0;

  useEffect(() => {
    if (phase !== "waiting") return;
    const tick = window.setInterval(() => setNow(Date.now()), TICK_MS);
    return () => window.clearInterval(tick);
  }, [phase]);

  useEffect(() => {
    if (phase !== "waiting" || expired) return;
    let cancelled = false;
    const timer = window.setInterval(async () => {
      try {
        const { devices } = await client.listDevices();
        const phone = devices.find((device) => !known.current.has(device.device_id));
        if (cancelled || !phone) return;
        setPaired(phone);
        setPhase("paired");
        onPaired?.(phone);
      } catch {
        // A missed poll is retried on the next tick; the code's own expiry ends the wait.
      }
    }, POLL_MS);
    return () => {
      cancelled = true;
      window.clearInterval(timer);
    };
  }, [client, phase, expired, onPaired]);

  function toggle(scope: CapabilityScope, on: boolean) {
    setScopes((current) => (on ? [...current, scope] : current.filter((item) => item !== scope)));
  }

  async function create() {
    if (!selected) return;
    setError(null);
    setPhase("creating");
    try {
      known.current = new Set((await client.listDevices()).devices.map((device) => device.device_id));
      const issued = await client.createPairing({
        name: name.trim() || "My phone",
        scopes: SCOPE_CHOICES.filter((choice) => choice.locked || scopes.includes(choice.id)).map((choice) => choice.id),
        kind: "person_device",
        via: selected.via,
      });
      setOffer(issued);
      setNow(Date.now());
      setPhase("waiting");
    } catch (cause) {
      setError(errorMessage(cause));
      setPhase("form");
      // 403 local_access_required means the proof went stale after the status was read.
      onStatusStale();
    }
  }

  function reset() {
    setOffer(null);
    setPaired(null);
    setError(null);
    setPhase("form");
  }

  async function copy(text: string, what: string) {
    toast.notify((await copyText(text)) ? `${what} copied` : "Could not copy; select the text instead", "success");
  }

  if (phase === "paired" && paired) {
    return (
      <div className="remote-result" role="status">
        <p className="remote-result-title">
          <Check size={18} strokeWidth={ICON_STROKE} aria-hidden="true" /> {paired.name || "New device"} is connected
        </p>
        <p className="remote-note">It can follow your tasks now. You can revoke it any time under Paired devices.</p>
        <div className="remote-actions">
          <Button variant="secondary" onClick={reset}>
            Connect another phone
          </Button>
          <Link className="ui-button ui-button--ghost" to="/devices" replace state={manageLinkState(location)}>
            See paired devices
          </Link>
        </div>
      </div>
    );
  }

  if ((phase === "waiting" || phase === "creating") && offer) {
    const payload = offer.url ?? offer.path;
    return (
      <div className="remote-offer" data-state={expired ? "expired" : "waiting"}>
        <div className="remote-qr" aria-hidden={expired ? true : undefined}>
          <QrCode text={payload} label={`QR code to connect ${name.trim() || "your phone"}`} size={208} />
          {expired && (
            <div className="remote-qr-veil">
              <span>Expired</span>
            </div>
          )}
        </div>
        <div className="remote-offer-copy">
          <ol className="remote-steps">
            <li>Open the camera on your phone and point it at the code.</li>
            <li>Tap the link that appears. This page notices when your phone connects.</li>
          </ol>
          <dl className="remote-facts">
            <div>
              <dt>Code</dt>
              <dd>
                <code>{offer.code}</code>
              </dd>
            </div>
            <div>
              <dt>{expired ? "Status" : "Expires in"}</dt>
              <dd>
                <span role="timer" aria-live="off" data-expired={expired || undefined}>
                  {expired ? "Expired" : countdown(remaining)}
                </span>
              </dd>
            </div>
            <div>
              <dt>Through</dt>
              <dd>{selected?.label}</dd>
            </div>
          </dl>
          {expired ? (
            <p className="remote-waiting">This code expired. Make a new one to continue.</p>
          ) : (
            <p className="remote-waiting" role="status">
              <Spinner size={16} />
              <span>Waiting for your phone…</span>
            </p>
          )}
          <div className="remote-actions">
            <Button variant={expired ? "primary" : "secondary"} onClick={() => void create()}>
              <RefreshCw size={16} strokeWidth={ICON_STROKE} aria-hidden="true" />
              {expired ? "Make a new code" : "New code"}
            </Button>
            {!expired && (
              <Button variant="ghost" aria-label="Copy the pairing link" onClick={() => void copy(payload, "Link")}>
                <Copy size={16} strokeWidth={ICON_STROKE} aria-hidden="true" />
                Copy link
              </Button>
            )}
            <Button variant="ghost" onClick={reset}>
              Cancel
            </Button>
          </div>
          {error && <ErrorNote>{error}</ErrorNote>}
        </div>
      </div>
    );
  }

  if (!status.pairing_allowed) {
    return <LocalOnlyNotice what="connect a phone" onChanged={onStatusStale} />;
  }
  if (options.length === 0) {
    return (
      <p className="remote-note">Turn on a connection first, then connect your phone through it.</p>
    );
  }

  return (
    <form
      className="remote-form"
      onSubmit={(event) => {
        event.preventDefault();
        void create();
      }}
    >
      <div className="remote-field">
        <label htmlFor={nameId}>Phone name</label>
        <input id={nameId} value={name} maxLength={60} onChange={(event) => setName(event.target.value)} autoComplete="off" />
      </div>
      {options.length > 1 ? (
        <div className="remote-field">
          <span className="remote-label">Connect through</span>
          <Segmented
            label="Connect through"
            options={options.map((option) => ({ value: String(option.via), label: option.label }))}
            value={String(selected?.via ?? null)}
            onChange={setVia}
          />
        </div>
      ) : (
        <p className="remote-note">
          <Smartphone size={14} strokeWidth={ICON_STROKE} aria-hidden="true" /> The phone connects through {selected?.label}.
        </p>
      )}
      <fieldset className="remote-scopes">
        <legend>What the phone may do</legend>
        {SCOPE_CHOICES.map((choice) => (
          <label key={choice.id} className="remote-scope" data-risk={choice.id === "admin" ? "high" : undefined}>
            <input
              type="checkbox"
              checked={choice.locked || scopes.includes(choice.id)}
              disabled={choice.locked}
              onChange={(event) => toggle(choice.id, event.target.checked)}
            />
            <span>{choice.label}</span>
          </label>
        ))}
      </fieldset>
      {error && <ErrorNote>{error}</ErrorNote>}
      <div className="remote-actions">
        <Button variant="primary" type="submit" disabled={phase === "creating"}>
          {phase === "creating" ? <Spinner size={16} /> : null}
          Show QR code
        </Button>
      </div>
    </form>
  );
}
