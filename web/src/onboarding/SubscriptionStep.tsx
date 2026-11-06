import { useCallback, useState } from "react";
import { useClient } from "../context";
import { useResource } from "../hooks/useResource";
import { t } from "../i18n";
import { ConnectDialog, MoreOptionsDialog, SubscriptionChoices, useSubscriptionChoices, type ConnectStart } from "../models/ConnectDialog";
import { Waiting } from "../models/OAuthDialogs";
import { KEY_PROVIDERS, providerLabel } from "../models/providers";
import { ErrorNote } from "../pages/blocks";
import { Frame, PrimaryButton } from "./Frame";
import type { StepNav } from "./summary";

/**
 * Aside's "Choose your AI subscription" with Settings › Models' own picker pieces. openside has no
 * plan of its own to fall back on, so the step stays skippable; the ghost button says Continue once
 * any provider already works.
 */
export function SubscriptionStep({ next, back, update }: StepNav) {
  const client = useClient();
  const { providers, featured, more } = useSubscriptionChoices();
  const connections = useResource(useCallback(() => client.modelProviders(), [client]));
  const [picked, setPicked] = useState<string | null>(null);
  const [showMore, setShowMore] = useState(false);
  const [connect, setConnect] = useState<ConnectStart | null>(null);
  const chosen = featured.find((choice) => choice.provider === picked) ?? featured[0];
  const ready = (connections.data?.providers ?? []).some((provider) => provider.status === "connected");

  async function connected(start: ConnectStart) {
    // The list names what actually works now; a key form may have saved a different provider than it opened on.
    const list = await client.modelProviders().catch(() => null);
    const names = (list?.providers ?? []).filter((provider) => provider.status === "connected").map((provider) => provider.display_name);
    update({ models: names.length ? names.join(", ") : start.kind === "subscription" ? start.name : providerLabel(start.provider) });
    next();
  }

  return (
    <Frame
      step="subscription"
      layout="centered"
      title={t("onboarding.subscription.title")}
      lead={t("onboarding.subscription.lead")}
      onBack={back}
      skip={{ label: ready ? t("common.continue") : t("onboarding.frame.skip"), onClick: next }}
      primary={
        <PrimaryButton disabled={!chosen} onClick={() => chosen && setConnect({ kind: "subscription", provider: chosen.provider, name: chosen.name })}>
          {t("onboarding.subscription.connect")}
        </PrimaryButton>
      }
    >
      <div className="onb-subscription">
        {providers.error ? <ErrorNote>{providers.error}</ErrorNote> : null}
        {providers.data ? (
          <SubscriptionChoices choices={featured} chosen={chosen?.provider} onPick={setPicked} />
        ) : providers.error ? null : (
          <Waiting text={t("models.connect.loading")} />
        )}
        <button type="button" className="models-more-link" onClick={() => setShowMore(true)}>
          {t("models.connect.more")}
        </button>
      </div>
      <MoreOptionsDialog
        open={showMore}
        choices={more}
        onClose={() => setShowMore(false)}
        onPick={(choice) => {
          setShowMore(false);
          setConnect({ kind: "subscription", provider: choice.provider, name: choice.name });
        }}
        onOwnKey={() => {
          setShowMore(false);
          setConnect({ kind: "api_key", provider: KEY_PROVIDERS[0] });
        }}
      />
      {connect ? <ConnectDialog open start={connect} onClose={() => setConnect(null)} onConnected={() => void connected(connect)} /> : null}
    </Frame>
  );
}
