import { useCallback, useEffect, useRef, useState } from "react";
import { ApiError } from "../../api/client";
import type { MemoryModelState, MemoryModelStatus, Retention } from "../../api/types";
import { useClient } from "../../context";
import { errorMessage, useResource } from "../../hooks/useResource";
import { t, type MessageKey } from "../../i18n";
import { tNodes } from "../../i18n/react";
import { ErrorNote } from "../../pages/blocks";
import { formatBytes, formatRelative } from "../../pages/format";
import { RestartNotice } from "../../pages/RestartNotice";
import { Badge, Button, cx, Select, SettingsCard, SettingsRow, SettingsSection, Switch, useToast } from "../../ui";
import { useMemoryConfig } from "./memoryConfig";
import "./memory.css";

const RETENTIONS: { value: Retention; label: MessageKey }[] = [
  { value: "permanent", label: "memory.configure.retention.permanent" },
  { value: "days_90", label: "memory.configure.retention.days90" },
  { value: "days_30", label: "memory.configure.retention.days30" },
];

const ACTIVE: MemoryModelState[] = ["resolving", "downloading", "verifying", "installing"];
const POLL_MS = 1_000;

const STAGE: Record<MemoryModelState, MessageKey> = {
  idle: "memory.model.stage.idle",
  resolving: "memory.model.stage.resolving",
  downloading: "memory.model.stage.downloading",
  verifying: "memory.model.stage.verifying",
  installing: "memory.model.stage.installing",
  installed: "memory.model.stage.installed",
  failed: "memory.model.stage.failed",
  cancelled: "memory.model.stage.cancelled",
};

function host(endpoint: string): string {
  try {
    return new URL(endpoint).host;
  } catch {
    return endpoint;
  }
}

function Transfer({ model }: { model: MemoryModelStatus }) {
  const known = model.bytes_total > 0;
  const stage = t(STAGE[model.state]);
  const status = [
    model.file ? t("memory.model.progressFile", { stage, file: model.file }) : stage,
    known ? t("memory.model.progressBytes", { done: formatBytes(model.bytes_done), total: formatBytes(model.bytes_total) }) : null,
    model.files_total > 0 ? t("memory.model.progressFiles", { current: Math.min(model.files_done + 1, model.files_total), total: model.files_total }) : null,
  ]
    .filter(Boolean)
    .join(" · ");
  return (
    <div className="memory-transfer">
      <progress aria-label={t("memory.model.progressLabel")} max={known ? model.bytes_total : undefined} value={known ? Math.min(model.bytes_done, model.bytes_total) : undefined} />
      <p>{status}</p>
    </div>
  );
}

/**
 * The local embedding model as a row under the memory status: download it on request, watch
 * the transfer, cancel it. The daemon never downloads on its own, so this is the only place
 * the download starts.
 */
function SearchModel({ onChanged }: { onChanged: () => void }) {
  const client = useClient();
  const load = useCallback(() => client.memoryModel(), [client]);
  const resource = useResource(load);
  const [model, setModel] = useState<MemoryModelStatus | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const requestGeneration = useRef(0);
  const changed = useRef(onChanged);
  changed.current = onChanged;
  const current = model ?? resource.data;
  const active = current ? ACTIVE.includes(current.state) : false;

  useEffect(() => {
    if (!active || busy) return;
    const generation = requestGeneration.current;
    let stopped = false;
    const timer = window.setInterval(() => {
      if (generation !== requestGeneration.current) return;
      client.memoryModel().then(
        (next) => {
          if (stopped || generation !== requestGeneration.current) return;
          setModel(next);
          if (!ACTIVE.includes(next.state)) changed.current();
        },
        () => {
          // A restarting daemon answers nothing for a moment; the next tick asks again.
        },
      );
    }, POLL_MS);
    return () => {
      stopped = true;
      window.clearInterval(timer);
    };
  }, [active, busy, client]);

  async function act(call: () => Promise<MemoryModelStatus>) {
    // A pre-action status response must not overwrite the download/cancel result.
    requestGeneration.current += 1;
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

  const installed = current.installed && !active && current.state !== "failed";
  const failure = current.error ? new ApiError(0, current.error.code, current.error.message).message : null;
  return (
    <div className="memory-row-group" role="region" aria-label={t("memory.model.title")}>
      <SettingsRow
        title={t("memory.model.title")}
        badge={<Badge>{t(STAGE[installed ? "installed" : current.state])}</Badge>}
        description={installed ? t("memory.model.detailInstalled") : t("memory.model.detailMissing")}
        control={
          active ? (
            <Button disabled={busy} onClick={() => void act(() => client.cancelMemoryModel())}>
              {t("common.cancel")}
            </Button>
          ) : installed ? undefined : (
            <Button disabled={busy} onClick={() => void act(() => client.downloadMemoryModel())}>
              {current.state === "failed" || current.state === "cancelled" ? t("common.tryAgain") : t("memory.model.download")}
            </Button>
          )
        }
      />
      <div className="memory-row-extra">
        {active ? <Transfer model={current} /> : null}
        {!active && !installed ? <p className="memory-note">{t("memory.model.source", { repo: current.source.repo, host: host(current.source.endpoint) })}</p> : null}
        {installed && current.model_id ? <p className="memory-note">{tNodes("memory.model.installedModel", { id: <code>{current.model_id}</code> })}</p> : null}
        {current.state === "cancelled" ? <p className="memory-note">{t("memory.model.cancelledNote")}</p> : null}
        {current.state === "failed" && failure ? <ErrorNote>{failure}</ErrorNote> : null}
        {error ? <ErrorNote>{error}</ErrorNote> : null}
        {current.restart_required ? (
          <RestartNotice
            onRestarted={() => {
              setModel(null);
              void resource.reload();
              changed.current();
            }}
          >
            {t("memory.model.restart")}
          </RestartNotice>
        ) : null}
      </div>
    </div>
  );
}

/**
 * Settings › Memory › Configure: memory status with Rebuild, the local search model, and the
 * `[memory]` settings. openside always generates memories from chats, so "Enable memories"
 * shows on and locked with a note (D6).
 */
export function MemoryConfigure() {
  const client = useClient();
  const toast = useToast();
  const config = useMemoryConfig();
  const loadHealth = useCallback(() => client.memoryHealth(), [client]);
  const health = useResource(loadHealth);
  const loadFiles = useCallback(() => client.memoryFiles(), [client]);
  const files = useResource(loadFiles);
  const [rebuilding, setRebuilding] = useState(false);

  const updated = (files.data?.files ?? [])
    .map((file) => file.updated_at)
    .filter((at): at is string => !!at)
    .sort()
    .at(-1);
  const status = health.data
    ? health.data.mode === "hybrid"
      ? t("memory.configure.status.ok")
      : t("memory.configure.status.keyword")
    : health.error
      ? t("memory.configure.status.unavailable")
      : "";
  const detail = health.data
    ? updated
      ? t("memory.configure.status.detail", { when: formatRelative(updated), count: health.data.count })
      : t("memory.configure.status.count", { count: health.data.count })
    : (health.error ?? undefined);

  async function rebuild() {
    setRebuilding(true);
    try {
      const result = await client.rebuildMemory();
      toast.notify(t("memory.configure.rebuilt", { count: result.reindexed }));
      await Promise.all([health.reload(), files.reload()]);
    } catch (cause) {
      toast.notify(errorMessage(cause), "error");
    } finally {
      setRebuilding(false);
    }
  }

  const retention = config.memory?.default_retention;
  return (
    <div className="memory-page" data-section="memory-configure">
      <SettingsSection label={t("memory.configure.memory")}>
        <SettingsCard>
          <div className="memory-row-group">
            <SettingsRow
              title={status}
              description={detail}
              control={
                <Button disabled={rebuilding || !health.data} onClick={() => void rebuild()}>
                  {rebuilding ? t("memory.configure.rebuilding") : t("memory.configure.rebuild")}
                </Button>
              }
            />
            {health.data ? (
              <div
                className={cx("memory-index-bar", rebuilding && "memory-index-bar--busy")}
                role="progressbar"
                aria-label={t("memory.configure.index")}
                aria-valuemin={0}
                aria-valuemax={100}
                aria-valuenow={rebuilding ? undefined : 100}
              />
            ) : null}
          </div>
          <SearchModel onChanged={() => void health.reload()} />
        </SettingsCard>
      </SettingsSection>
      <SettingsSection label={t("memory.configure.settings")}>
        <SettingsCard>
          <SettingsRow
            title={t("memory.configure.enable.title")}
            description={t("memory.configure.enable.description")}
            control={(ids) => <Switch aria-labelledby={ids.labelId} aria-describedby={ids.descriptionId} checked disabled onChange={() => undefined} />}
          />
          <SettingsRow
            title={t("memory.configure.retention.title")}
            description={t("memory.configure.retention.description")}
            control={(ids) => (
              <Select
                variant="inline"
                aria-labelledby={ids.labelId}
                aria-describedby={ids.descriptionId}
                disabled={!config.memory || config.busy}
                value={retention ?? ""}
                placeholder="—"
                options={RETENTIONS.map((option) => ({ value: option.value, label: t(option.label) }))}
                onChange={(value) => void config.set("default_retention", value as Retention)}
              />
            )}
          />
        </SettingsCard>
        {config.error ? <ErrorNote>{config.error}</ErrorNote> : null}
        {config.restartRequired ? <RestartNotice onRestarted={() => void config.reload()}>{t("memory.restart")}</RestartNotice> : null}
      </SettingsSection>
    </div>
  );
}
