import { Download } from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";
import { ApiError } from "../api/client";
import type { MemoryModelState, MemoryModelStatus } from "../api/types";
import { useClient } from "../context";
import { errorMessage, useResource } from "../hooks/useResource";
import { Button } from "../ui";
import { ErrorNote, ResourceCard, type BadgeTone } from "./blocks";
import { formatBytes } from "./format";
import { RestartNotice } from "./RestartNotice";

const ACTIVE: MemoryModelState[] = ["resolving", "downloading", "verifying", "installing"];

const STAGE: Record<MemoryModelState, { label: string; tone: BadgeTone }> = {
  idle: { label: "Not installed", tone: "neutral" },
  resolving: { label: "Preparing", tone: "attention" },
  downloading: { label: "Downloading", tone: "attention" },
  verifying: { label: "Checking", tone: "attention" },
  installing: { label: "Installing", tone: "attention" },
  installed: { label: "Installed", tone: "success" },
  failed: { label: "Failed", tone: "danger" },
  cancelled: { label: "Cancelled", tone: "neutral" },
};

function host(endpoint: string): string {
  try {
    return new URL(endpoint).host;
  } catch {
    return endpoint;
  }
}

function Progress({ model }: { model: MemoryModelStatus }) {
  const known = model.bytes_total > 0;
  const stage = model.state === "downloading" ? "Downloading" : STAGE[model.state].label;
  return (
    <div className="model-progress">
      <progress
        aria-label="Model download"
        max={known ? model.bytes_total : undefined}
        value={known ? Math.min(model.bytes_done, model.bytes_total) : undefined}
      />
      <p role="status">
        {stage}
        {model.file ? ` ${model.file}` : ""}
        {known ? ` · ${formatBytes(model.bytes_done)} of ${formatBytes(model.bytes_total)}` : ""}
        {model.files_total > 0 ? ` · file ${Math.min(model.files_done + 1, model.files_total)} of ${model.files_total}` : ""}
      </p>
    </div>
  );
}

/**
 * The local embedding model: download it on request, watch the transfer, cancel it. The daemon
 * never downloads on its own, so this card is the only place the download starts.
 */
export function MemoryModelCard({ onChanged, pollMs = 1_000 }: { onChanged?: () => void; pollMs?: number }) {
  const client = useClient();
  const load = useCallback(() => client.memoryModel(), [client]);
  const resource = useResource(load);
  const [model, setModel] = useState<MemoryModelStatus | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const changed = useRef(onChanged);
  changed.current = onChanged;
  const current = model ?? resource.data;
  const active = current ? ACTIVE.includes(current.state) : false;

  useEffect(() => {
    if (!active) return;
    let stopped = false;
    const timer = window.setInterval(() => {
      client.memoryModel().then(
        (next) => {
          if (stopped) return;
          setModel(next);
          if (!ACTIVE.includes(next.state)) changed.current?.();
        },
        () => {
          // A restarting daemon answers nothing for a moment; the next tick asks again.
        },
      );
    }, pollMs);
    return () => {
      stopped = true;
      window.clearInterval(timer);
    };
  }, [active, client, pollMs]);

  async function act(call: () => Promise<MemoryModelStatus>) {
    setBusy(true);
    setError(null);
    try {
      setModel(await call());
    } catch (cause) {
      setError(errorMessage(cause));
      // Another browser may have started it already; the daemon's answer is the truth.
      if (cause instanceof ApiError && cause.code === "download_in_progress") void client.memoryModel().then(setModel, () => undefined);
    } finally {
      setBusy(false);
    }
  }

  if (resource.forbidden) return null;
  if (!current) return resource.error ? <ErrorNote>{resource.error}</ErrorNote> : null;

  const stage = STAGE[current.state];
  const installed = current.installed && !active && current.state !== "failed";
  const badge = installed ? STAGE.installed : stage;
  const failure = current.error ? new ApiError(0, current.error.code, current.error.message).message : null;

  return (
    <section className="model-card" aria-label="Search model">
      <ResourceCard
        icon={Download}
        title="Search model"
        badge={badge.label}
        badgeTone={badge.tone}
        detail={
          installed
            ? "A small model on this computer helps memory search find related wording, not just the same words."
            : "Without it, memory search matches keywords only. The model is small and runs on this computer."
        }
        trailing={
          active ? (
            <Button size="compact" disabled={busy} onClick={() => void act(() => client.cancelMemoryModel())}>
              Cancel
            </Button>
          ) : installed ? undefined : (
            <Button size="compact" variant="primary" disabled={busy} onClick={() => void act(() => client.downloadMemoryModel())}>
              {current.state === "failed" || current.state === "cancelled" ? "Try again" : "Download model"}
            </Button>
          )
        }
      >
        {active ? <Progress model={current} /> : null}
        {!active && !installed ? (
          <p className="resource-card-body">
            From {current.source.repo} on {host(current.source.endpoint)}. Every file is checked against its published checksum before it is used.
          </p>
        ) : null}
        {installed && current.model_id ? (
          <p className="resource-card-body">
            Model <code>{current.model_id}</code> is installed.
          </p>
        ) : null}
        {current.state === "cancelled" ? <p className="resource-card-body">The download was cancelled. Nothing was installed.</p> : null}
        {current.state === "failed" && failure ? <ErrorNote>{failure}</ErrorNote> : null}
        {error ? <ErrorNote>{error}</ErrorNote> : null}
        {current.restart_required ? (
          <RestartNotice
            onRestarted={() => {
              setModel(null);
              void resource.reload();
              changed.current?.();
            }}
          >
            Restart openside to start using the model for memory search.
          </RestartNotice>
        ) : null}
      </ResourceCard>
    </section>
  );
}
