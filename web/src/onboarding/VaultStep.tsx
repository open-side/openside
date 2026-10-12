import { KeyRound } from "lucide-react";
import { useCallback, useEffect } from "react";
import { Link, useLocation } from "react-router";
import type { VaultStatus } from "../api/types";
import { useClient } from "../context";
import { useResource } from "../hooks/useResource";
import { manageLinkState } from "../pages/manageRoutes";
import { ResourceCard, ResourceView, type BadgeTone } from "../pages/blocks";
import type { StepProps } from "./summary";

/** The Vault dialog opens over this page; there is no event when it closes, so look again. */
const REFRESH_MS = 2_500;

const STATE: Record<VaultStatus["state"], { label: string; tone: BadgeTone; detail: string }> = {
  uninitialized: { label: "Not set up", tone: "neutral", detail: "Choose how the vault unlocks, then add the logins the agent may use." },
  locked: { label: "Locked", tone: "neutral", detail: "Set up. Unlock it from the Vault when a login is needed." },
  unlocked: { label: "Ready", tone: "success", detail: "Set up and unlocked." },
  unavailable: { label: "Unavailable", tone: "attention", detail: "The vault cannot be used on this computer yet. The Vault page says why." },
};

export function VaultStep({ update }: StepProps) {
  const client = useClient();
  const location = useLocation();
  const load = useCallback(() => client.vaultStatus(), [client]);
  const status = useResource(load);
  const { data, reload } = status;

  useEffect(() => {
    const timer = window.setInterval(() => void reload(), REFRESH_MS);
    return () => window.clearInterval(timer);
  }, [reload]);

  useEffect(() => {
    if (data) update({ vaultReady: data.state === "locked" || data.state === "unlocked" });
  }, [data, update]);

  return (
    <ResourceView resource={status} what="the vault">
      {(vault) => {
        const state = STATE[vault.state];
        const ready = vault.state === "locked" || vault.state === "unlocked";
        return (
          <ResourceCard
            icon={KeyRound}
            title="Vault"
            badge={state.label}
            badgeTone={state.tone}
            detail={state.detail}
            trailing={
              <Link
                to="/vault"
                state={manageLinkState(location)}
                className={`ui-button ui-button--compact ${ready ? "" : "ui-button--primary"}`}
                aria-label={ready ? "Open the vault" : "Set up the vault"}
              >
                {ready ? "Open" : "Set up"}
              </Link>
            }
          />
        );
      }}
    </ResourceView>
  );
}
