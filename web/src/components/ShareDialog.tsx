import { Copy, Trash2 } from "lucide-react";
import { useEffect, useState } from "react";
import { ApiError } from "../api/client";
import type { ShareLink } from "../api/types";
import { useClient } from "../context";
import { t, tMaybe } from "../i18n";
import { formatDateTime } from "../pages/format";
import { Button, copyText, Dialog, DialogActions, Dropdown, ICON_STROKE, Select, Spinner, Switch, useToast } from "../ui";
import "./share-dialog.css";

const icon = { size: 16, strokeWidth: ICON_STROKE, "aria-hidden": true } as const;
// The daemon's MAX_SHARE_ARTIFACTS.
const MAX_FILES = 20;
// The daemon serves the transcript only to the chat's own devices; it cannot be shared.
const TRANSCRIPT = "transcript.jsonl";
/** Images are re-encoded without metadata and shared pixel for pixel; other binaries are refused. */
const IMAGE = /\.(png|jpe?g|gif|webp)$/i;

/** The chosen images as they will be shared, so the person sees what leaves the computer. */
function ImagePreviews({ sessionId, paths }: { sessionId: string; paths: string[] }) {
  const client = useClient();
  const [urls, setUrls] = useState<Record<string, string>>({});
  const key = paths.join("\n");
  useEffect(() => {
    let live = true;
    const made: string[] = [];
    for (const path of paths) {
      client.artifactBlob(sessionId, path).then(
        (blob) => {
          if (!live) return;
          const url = URL.createObjectURL(blob);
          made.push(url);
          setUrls((current) => ({ ...current, [path]: url }));
        },
        () => undefined,
      );
    }
    return () => {
      live = false;
      made.forEach((url) => URL.revokeObjectURL(url));
      setUrls({});
    };
    // `key` stands for `paths`, whose identity changes on every render.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [client, sessionId, key]);
  if (paths.length === 0) return null;
  return (
    <div className="share-images">
      <p role="note" className="share-hint">
        {t("share.dialog.imageWarning")}
      </p>
      <ul>
        {paths.map((path) => (
          <li key={path}>{urls[path] ? <img src={urls[path]} alt={path.split("/").pop() ?? path} /> : <Spinner size={14} />}</li>
        ))}
      </ul>
    </div>
  );
}
const EXPIRY_SECS: Record<string, number | null> = { never: null, day: 86_400, week: 7 * 86_400, month: 30 * 86_400 };

function shareError(cause: unknown): string {
  if (!(cause instanceof ApiError)) return t("shell.requestFailed");
  return tMaybe(`share.error.${cause.code}`) ?? cause.message;
}

/** A link opens through remote access when one is set up; otherwise only on this computer. */
function linkOf(link: ShareLink): string {
  return link.url ?? `${window.location.origin}${link.path}`;
}

/** Chat menu → Share: publish a read-only snapshot of the chat and manage its links. */
export function ShareDialog({ sessionId, onClose }: { sessionId: string; onClose: () => void }) {
  const client = useClient();
  const toast = useToast();
  const [links, setLinks] = useState<ShareLink[] | null>(null);
  const [files, setFiles] = useState<string[]>([]);
  const [chosen, setChosen] = useState<string[]>([]);
  const [steps, setSteps] = useState(true);
  const [expiry, setExpiry] = useState("never");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let live = true;
    client.listShares(sessionId).then(
      (list) => live && setLinks(list.shares),
      (cause: unknown) => {
        if (!live) return;
        setLinks([]);
        setError(shareError(cause));
      },
    );
    client.listArtifacts(sessionId).then(
      (list) => live && setFiles(list.artifacts.map((artifact) => artifact.path).filter((path) => path !== TRANSCRIPT)),
      () => undefined,
    );
    return () => {
      live = false;
    };
  }, [client, sessionId]);

  async function copy(link: ShareLink, done: "share.toast.copied" | "share.toast.createdCopied") {
    if (await copyText(linkOf(link))) toast.notify(t(done));
    else toast.notify(t(done === "share.toast.copied" ? "share.toast.copyFailed" : "share.toast.created"), "error");
  }

  async function create() {
    setBusy(true);
    setError(null);
    try {
      const link = await client.createShare(sessionId, { artifacts: chosen, steps, expires_in_secs: EXPIRY_SECS[expiry] ?? null });
      setLinks((current) => [link, ...(current ?? [])]);
      setChosen([]);
      await copy(link, "share.toast.createdCopied");
    } catch (cause) {
      setError(shareError(cause));
    } finally {
      setBusy(false);
    }
  }

  async function revoke(link: ShareLink) {
    setError(null);
    try {
      await client.revokeShare(sessionId, link.id);
      setLinks((current) => (current ?? []).filter((other) => other.id !== link.id));
      toast.notify(t("share.toast.revoked"));
    } catch (cause) {
      setError(shareError(cause));
    }
  }

  return (
    <Dialog open onClose={() => !busy && onClose()} title={t("share.dialog.title")} description={t("share.dialog.description")}>
      <div className="dialog-form share-dialog">
        <div className="share-option">
          <div>
            <span id="share-steps-label">{t("share.dialog.steps")}</span>
            <p id="share-steps-hint">{t("share.dialog.stepsHint")}</p>
          </div>
          <Switch checked={steps} onChange={setSteps} aria-labelledby="share-steps-label" aria-describedby="share-steps-hint" />
        </div>
        <label className="field" htmlFor="share-expiry">
          <span>{t("share.dialog.expiry")}</span>
          <Select
            id="share-expiry"
            variant="field"
            value={expiry}
            onChange={setExpiry}
            options={(["never", "day", "week", "month"] as const).map((value) => ({ value, label: t(`share.expiry.${value}` as const) }))}
          />
        </label>
        {files.length ? (
          <div className="field">
            <span id="share-files-label">{t("share.dialog.files")}</span>
            <Dropdown
              multiple
              variant="field"
              aria-labelledby="share-files-label"
              aria-describedby="share-files-hint"
              placeholder={t("share.dialog.filesPlaceholder")}
              value={chosen}
              onChange={(next) => setChosen(next.slice(0, MAX_FILES))}
              options={files.map((path) => ({ value: path, label: path.split("/").pop() ?? path, detail: path }))}
            />
            <p id="share-files-hint" className="share-hint">
              {t("share.dialog.filesHint", { max: MAX_FILES })}
            </p>
            <ImagePreviews sessionId={sessionId} paths={chosen.filter((path) => IMAGE.test(path))} />
          </div>
        ) : null}
        {error ? (
          <p role="alert" className="error-note">
            {error}
          </p>
        ) : null}
        <section className="share-links" aria-labelledby="share-links-label">
          <h3 id="share-links-label">{t("share.dialog.links")}</h3>
          {links === null ? (
            <p role="status" className="share-hint">
              <Spinner size={14} /> {t("share.dialog.loading")}
            </p>
          ) : links.length === 0 ? (
            <p className="share-hint">{t("share.dialog.none")}</p>
          ) : (
            <>
              {links.some((link) => !link.url) ? <p className="share-hint">{t("share.dialog.localOnly")}</p> : null}
              <ul>
                {links.map((link) => (
                  <li key={link.id}>
                    <span className="share-link-meta">
                      {[
                        t("share.dialog.shared", { date: formatDateTime(link.created_at) }),
                        link.expires_at ? t("share.dialog.expires", { date: formatDateTime(link.expires_at) }) : t("share.dialog.noExpiry"),
                        t("share.dialog.messages", { count: link.messages }),
                      ].join(" · ")}
                    </span>
                    <Button size="small" onClick={() => void copy(link, "share.toast.copied")}>
                      <Copy {...icon} />
                      {t("share.dialog.copy")}
                    </Button>
                    <Button size="small" variant="danger" onClick={() => void revoke(link)}>
                      <Trash2 {...icon} />
                      {t("share.dialog.revoke")}
                    </Button>
                  </li>
                ))}
              </ul>
            </>
          )}
        </section>
        <DialogActions>
          <Button disabled={busy} onClick={onClose}>
            {t("common.close")}
          </Button>
          <Button variant="primary" disabled={busy || links === null} onClick={() => void create()}>
            {busy ? t("share.dialog.creating") : t("share.dialog.create")}
          </Button>
        </DialogActions>
      </div>
    </Dialog>
  );
}
