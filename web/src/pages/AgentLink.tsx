import { Check, Copy } from "lucide-react";
import { useEffect, useState } from "react";
import type { PairingOffer } from "../api/types";
import { ApiError } from "../api/client";
import { useClient } from "../context";
import { errorMessage } from "../hooks/useResource";
import { LocalOnlyNotice } from "../remote/LocalOnlyNotice";
import { Button, copyText, ICON_STROKE, useToast } from "../ui";
import { CliHint } from "./CliHint";
import { ErrorNote } from "./blocks";

const AGENT_SCOPES = ["read", "steer", "browse", "approve"] as const;

function secondsLeft(offer: PairingOffer, now: number): number {
  return Math.max(0, Math.ceil((Date.parse(offer.expires_at) - now) / 1000));
}

/** Issues a one-time link for an external CLI or MCP agent; the agent redeems it on its own computer. */
export function AgentLink() {
  const client = useClient();
  const toast = useToast();
  const [name, setName] = useState("My agent");
  const [offer, setOffer] = useState<PairingOffer | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<{ message: string; local: boolean } | null>(null);
  const [now, setNow] = useState(() => Date.now());

  useEffect(() => {
    if (!offer) return;
    const timer = window.setInterval(() => setNow(Date.now()), 1_000);
    return () => window.clearInterval(timer);
  }, [offer]);

  async function create() {
    setBusy(true);
    setError(null);
    try {
      setOffer(await client.createPairing({ name: name.trim() || "My agent", scopes: [...AGENT_SCOPES], kind: "agent_client", via: null }));
      setNow(Date.now());
    } catch (cause) {
      setOffer(null);
      setError({ message: errorMessage(cause), local: cause instanceof ApiError && cause.code === "local_access_required" });
    } finally {
      setBusy(false);
    }
  }

  const link = offer ? (offer.url ?? offer.code) : null;
  const left = offer ? secondsLeft(offer, now) : 0;

  async function copy() {
    if (link) toast.notify((await copyText(link)) ? "Link copied" : "Could not copy; select the link instead", "success");
  }

  return (
    <div className="agent-link">
      <p className="section-list-description">
        Connect a coding agent or script that runs on its own computer. It signs in with a one-time link. It waits for a person’s approval on one of your devices before it does anything sensitive, and the
        link fixes that role.
      </p>
      <div className="agent-link-form">
        <label className="field">
          <span>Agent name</span>
          <input value={name} maxLength={100} onChange={(event) => setName(event.target.value)} />
        </label>
        <Button variant="primary" disabled={busy} onClick={() => void create()}>
          {busy ? "Creating…" : offer ? "Create a new link" : "Create a link"}
        </Button>
      </div>
      {error?.local ? <LocalOnlyNotice what="connect an agent" onChanged={() => void create()} /> : error ? <ErrorNote>{error.message}</ErrorNote> : null}
      {offer && link ? (
        left > 0 ? (
          <div className="agent-link-result" role="status">
            <p className="onb-result-title">
              <Check size={16} strokeWidth={ICON_STROKE} aria-hidden="true" /> Link ready · expires in {Math.floor(left / 60)}:{String(left % 60).padStart(2, "0")}
            </p>
            <div className="remote-copy-row">
              <input readOnly aria-label="Agent link" value={link} onFocus={(event) => event.currentTarget.select()} />
              <Button size="compact" aria-label="Copy the agent link" onClick={() => void copy()}>
                <Copy size={16} strokeWidth={ICON_STROKE} aria-hidden="true" /> Copy
              </Button>
            </div>
            <p className="section-list-description">
              Give it to the agent. It uses it once, on its own computer, to sign in; the agent’s setup tells it where to paste a link.
            </p>
            <CliHint command="openside connect">On the agent’s computer, the command that takes this link is</CliHint>
          </div>
        ) : (
          <ErrorNote>This link has expired. Create a new one.</ErrorNote>
        )
      ) : null}
    </div>
  );
}
