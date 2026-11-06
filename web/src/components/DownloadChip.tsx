import { CircleAlert, Download, File, TriangleAlert } from "lucide-react";
import { ApiError } from "../api/client";
import { useClient } from "../context";
import { t, type MessageKey } from "../i18n";
import type { TimelineItem } from "../state/session";
import { Button, ICON_STROKE, Spinner, useToast } from "../ui";
import { download, size } from "./FilesTray";
import "./documents.css";

type DownloadItem = Extract<TimelineItem, { type: "download" }>;

const FAILURE: Record<string, MessageKey> = {
  canceled: "timeline.download.failed.canceled",
  too_large: "timeline.download.failed.tooLarge",
  session_quota: "timeline.download.failed.sessionQuota",
  policy: "timeline.download.failed.policy",
  unavailable: "timeline.download.failed.unavailable",
};

function hostOf(url: string): string | null {
  try {
    return new URL(url).host || null;
  } catch {
    return null;
  }
}

/**
 * A file the agent's browsing downloaded, as a chip in the thread. The file is only ever saved
 * to the device through the authenticated raw route, never previewed here: its bytes came from
 * an arbitrary site.
 */
export function DownloadChip({ sessionId, item }: { sessionId: string; item: DownloadItem }) {
  const client = useClient();
  const toast = useToast();
  const host = hostOf(item.url);
  const icon = { size: 20, strokeWidth: ICON_STROKE, "aria-hidden": true } as const;

  if (item.state === "in_progress" || item.state === "saving") {
    const label = item.state === "saving" ? t("timeline.download.saving") : host ? t("timeline.download.downloading", { host }) : t("timeline.download.downloadingAny");
    return (
      <div className="download-chip" data-state={item.state}>
        <Spinner size={18} />
        <span className="download-text">
          <span className="download-name">{label}</span>
          {item.state === "saving" && host ? <span className="download-meta">{host}</span> : null}
        </span>
      </div>
    );
  }

  if (item.state === "failed" || !item.artifact) {
    return (
      <div className="download-chip" data-state="failed">
        <CircleAlert {...icon} />
        <span className="download-text">
          <span className="download-name">{t(item.error && Object.hasOwn(FAILURE, item.error) ? FAILURE[item.error]! : "timeline.download.failed.unavailable")}</span>
          {host ? <span className="download-meta">{host}</span> : null}
        </span>
      </div>
    );
  }

  const path = item.artifact;
  const name = item.fileName ?? path.split("/").pop() ?? path;
  const meta = [item.size !== undefined ? size(item.size) : null, host].filter(Boolean).join(" · ");
  return (
    <div className="download-chip" data-state="saved" data-quarantined={item.quarantined || undefined}>
      <File {...icon} />
      <span className="download-text">
        <span className="download-name" title={name}>
          {name}
        </span>
        {meta ? <span className="download-meta">{meta}</span> : null}
        {item.quarantined ? (
          <span className="download-caution">
            <TriangleAlert size={14} strokeWidth={ICON_STROKE} aria-hidden="true" />
            {t("timeline.download.quarantined")}
          </span>
        ) : null}
      </span>
      <Button
        size="small"
        aria-label={t("thread.files.downloadFile", { name })}
        onClick={() =>
          void download(client, sessionId, path).catch((cause) => toast.notify(cause instanceof ApiError ? cause.message : t("thread.requestFailed"), "error"))
        }
      >
        <Download size={16} strokeWidth={ICON_STROKE} aria-hidden="true" />
        {t("thread.files.download")}
      </Button>
    </div>
  );
}
