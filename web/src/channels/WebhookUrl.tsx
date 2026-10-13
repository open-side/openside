import { Copy } from "lucide-react";
import type { ChannelAccount } from "../api/types";
import { Button, copyText, IconButton, ICON_STROKE, useToast } from "../ui";
import { isLoopbackUrl, serviceOf } from "./services";

/** The signed address Slack or Discord posts to, with Copy; managed accounts only. */
export function WebhookUrl({ account, onRemote }: { account: ChannelAccount; onRemote: () => void }) {
  const toast = useToast();
  const url = account.webhook_url;
  const callback = serviceOf(account.kind).callback;
  if (!url || !callback) return null;

  return (
    <div className="channel-callback">
      <span className="channel-callback-label">{callback.field}</span>
      <span className="channel-url">
        <code>{url}</code>
        <IconButton
          label={`Copy callback address for ${account.label}`}
          size="small"
          onClick={() => void copyText(url).then((copied) => toast.notify(copied ? "Address copied." : "Could not copy. Select the address and copy it.", copied ? undefined : "error"))}
        >
          <Copy size={16} strokeWidth={ICON_STROKE} aria-hidden="true" />
        </IconButton>
      </span>
      {isLoopbackUrl(url) ? (
        <p className="channel-note">
          This address only works on this computer, so {serviceOf(account.kind).name} cannot reach it yet.{" "}
          <Button variant="ghost" size="compact" onClick={onRemote}>
            Set up remote access
          </Button>
        </p>
      ) : null}
    </div>
  );
}
