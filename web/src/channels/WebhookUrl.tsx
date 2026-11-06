import { Copy } from "lucide-react";
import { useNavigate } from "react-router";
import type { ChannelAccount } from "../api/types";
import { t } from "../i18n";
import { settingsPath } from "../settings/nav";
import { Button, copyText, IconButton, ICON_STROKE, useToast } from "../ui";
import { isLoopbackUrl, serviceOf } from "./services";

/**
 * The signed address Slack or Discord posts to, with Copy; managed accounts only. A loopback
 * address points at Settings › Developers, where Remote Control publishes this computer. With
 * Remote Control off the daemon withholds the address, and this says how to get it back.
 */
export function WebhookUrl({ account, remoteOff = false }: { account: ChannelAccount; remoteOff?: boolean }) {
  const toast = useToast();
  const navigate = useNavigate();
  const url = account.webhook_url;
  const callback = serviceOf(account.kind).callback;
  if (!callback) return null;
  if (remoteOff) {
    return (
      <div className="channel-callback">
        <p className="channel-note">
          {t("models.channels.webhook.remoteOff")}{" "}
          <Button variant="ghost" onClick={() => navigate(settingsPath("developers"))}>
            {t("models.channels.webhook.remote")}
          </Button>
        </p>
      </div>
    );
  }
  if (!url) return null;

  return (
    <div className="channel-callback">
      <span className="channel-callback-label">{callback.field}</span>
      <span className="channel-url">
        <code>{url}</code>
        <IconButton
          label={t("models.channels.webhook.copy", { name: account.label })}
          size="small"
          onClick={() => void copyText(url).then((copied) => toast.notify(copied ? t("models.channels.webhook.copied") : t("models.channels.webhook.copyFailed"), copied ? undefined : "error"))}
        >
          <Copy size={16} strokeWidth={ICON_STROKE} aria-hidden="true" />
        </IconButton>
      </span>
      {isLoopbackUrl(url) ? (
        <p className="channel-note">
          {t("models.channels.webhook.loopback", { service: serviceOf(account.kind).name })}{" "}
          <Button variant="ghost" onClick={() => navigate(settingsPath("developers"))}>
            {t("models.channels.webhook.remote")}
          </Button>
        </p>
      ) : null}
    </div>
  );
}
