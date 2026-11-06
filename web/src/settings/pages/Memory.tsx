import { Clock, FileText, FolderDown, FolderOpen } from "lucide-react";
import { useCallback, useEffect, useId, useState } from "react";
import { ApiError } from "../../api/client";
import type { MemoryFile, MemoryFileInfo } from "../../api/types";
import { Markdown } from "../../components/Markdown";
import { useClient } from "../../context";
import { errorMessage, useResource } from "../../hooks/useResource";
import { t } from "../../i18n";
import { ErrorNote, Loading } from "../../pages/blocks";
import { formatDateTime, formatRelative } from "../../pages/format";
import { ScopeNotice } from "../../pages/ScopeNotice";
import { Button, cx, ICON_STROKE, useToast } from "../../ui";
import "./memory.css";

const FILE_ICON = { size: 14, strokeWidth: ICON_STROKE, "aria-hidden": true } as const;

// The daemon's note markers (`opensided/src/memory/views.rs`): a whole line, flush left.
const MARKER_LINE = /^<!-- memory(?:[ \t][^\n]*?)?-->[ \t\r]*(?:\n|$)/gm;

/** The file as a reader sees it: the note markers only matter to the editor. */
function withoutMarkers(markdown: string): string {
  return markdown.replace(MARKER_LINE, "");
}

function saveAsFile(name: string, markdown: string) {
  const url = URL.createObjectURL(new Blob([markdown], { type: "text/markdown" }));
  const link = document.createElement("a");
  link.href = url;
  link.download = name;
  link.click();
  setTimeout(() => URL.revokeObjectURL(url), 1000);
}

function Updated({ at }: { at: string | null | undefined }) {
  if (!at) return null;
  return (
    <span className="memory-updated" title={t("memory.overview.updated", { when: formatDateTime(at) })}>
      <Clock size={12} strokeWidth={ICON_STROKE} aria-hidden="true" />
      <time dateTime={at}>{formatRelative(at)}</time>
    </span>
  );
}

/** The right pane: one file rendered, or its raw markdown in the editor after Edit. */
function FilePane({ info, onSaved }: { info: MemoryFileInfo; onSaved: () => void }) {
  const client = useClient();
  const toast = useToast();
  const headingId = useId();
  const name = info.name;
  const load = useCallback(() => client.memoryFile(name), [client, name]);
  const loaded = useResource(load);
  const [file, setFile] = useState<MemoryFile | null>(null);
  const [draft, setDraft] = useState<string | null>(null);
  const [failure, setFailure] = useState<string | null>(null);
  const [conflict, setConflict] = useState(false);
  const [busy, setBusy] = useState(false);
  const current = file ?? loaded.data;

  function edit() {
    if (!current) return;
    setDraft(current.markdown);
    setFailure(null);
    setConflict(false);
  }

  function cancel() {
    setDraft(null);
    setFailure(null);
    setConflict(false);
  }

  async function save() {
    if (!current || draft === null) return;
    setBusy(true);
    setFailure(null);
    try {
      const saved = await client.updateMemoryFile(name, { markdown: draft, revision: current.revision });
      setFile(saved.file);
      setDraft(null);
      toast.notify(t("memory.overview.saved", { name }));
      onSaved();
    } catch (cause) {
      if (cause instanceof ApiError && cause.code === "memory_conflict") setConflict(true);
      else setFailure(errorMessage(cause));
    } finally {
      setBusy(false);
    }
  }

  // After a conflict the edit cannot be merged safely; start again from what is stored now.
  async function reload() {
    setBusy(true);
    try {
      const latest = await client.memoryFile(name);
      setFile(latest);
      setDraft(latest.markdown);
      setConflict(false);
      setFailure(null);
    } catch (cause) {
      setFailure(errorMessage(cause));
    } finally {
      setBusy(false);
    }
  }

  const editing = draft !== null;
  const shown = current ? withoutMarkers(current.markdown) : "";
  return (
    <section className="memory-detail-pane" aria-labelledby={headingId}>
      <header className="memory-pane-head memory-detail-head">
        <FileText {...FILE_ICON} />
        <h2 className="memory-detail-title" id={headingId}>
          {name}
        </h2>
        <Updated at={current?.updated_at ?? info.updated_at} />
        {editing ? (
          <>
            <Button size="small" onClick={cancel} disabled={busy}>
              {t("common.cancel")}
            </Button>
            <Button variant="primary" className="memory-head-button" onClick={() => void save()} disabled={busy || conflict}>
              {t("common.save")}
            </Button>
          </>
        ) : current?.editable ? (
          <Button variant="primary" className="memory-head-button" onClick={edit}>
            {t("common.edit")}
          </Button>
        ) : null}
      </header>
      <div className="memory-detail-body">
        <div className="memory-doc">
          {loaded.error && !current ? <ErrorNote>{loaded.error}</ErrorNote> : null}
          {!current ? (
            loaded.error ? null : <Loading />
          ) : editing ? (
            <div className="memory-editor">
              <p className="memory-note">{t("memory.overview.markerHelp")}</p>
              {conflict ? (
                <div className="memory-conflict" role="alert">
                  <p>{t("memory.overview.conflict")}</p>
                  <Button size="small" onClick={() => void reload()} disabled={busy}>
                    {t("memory.overview.reload")}
                  </Button>
                </div>
              ) : null}
              {failure ? <ErrorNote>{failure}</ErrorNote> : null}
              <textarea
                className="memory-textarea"
                aria-label={t("memory.overview.editorLabel", { name })}
                value={draft}
                spellCheck={false}
                onChange={(event) => setDraft(event.target.value)}
              />
            </div>
          ) : (
            <>
              <Markdown text={shown} />
              {info.items === 0 ? <p className="memory-note">{t("memory.overview.empty")}</p> : null}
            </>
          )}
        </div>
      </div>
    </section>
  );
}

/**
 * Settings › Memory › Overview (D5): the store as MEMORY.md, USER.md and TAXONOMY.md, one file
 * open on the right. On the computer running openside "Open folder" writes a copy of the files
 * and shows it in the file manager; other devices cannot reach that folder, so they download.
 */
export function MemoryOverview() {
  const client = useClient();
  const toast = useToast();
  const load = useCallback(() => client.memoryFiles(), [client]);
  const files = useResource(load);
  const [selected, setSelected] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  // The settings document's `remote` is the daemon's verdict on this browser's local authority;
  // null until it answers, so the button does not switch under the pointer.
  const [local, setLocal] = useState<boolean | null>(null);
  const list = files.data?.files ?? [];
  const open = list.find((file) => file.name === selected) ?? list[0];

  useEffect(() => {
    let live = true;
    client.settingsConfig().then(
      (settings) => live && setLocal(!settings.remote),
      () => live && setLocal(false),
    );
    return () => {
      live = false;
    };
  }, [client]);

  async function download() {
    setBusy(true);
    try {
      const all = await Promise.all(list.map((file) => client.memoryFile(file.name)));
      for (const file of all) saveAsFile(file.name, file.markdown);
      toast.notify(t("memory.overview.downloaded"));
    } catch (cause) {
      toast.notify(errorMessage(cause), "error");
    } finally {
      setBusy(false);
    }
  }

  async function reveal() {
    setBusy(true);
    try {
      await client.revealMemoryFolder();
    } catch (cause) {
      toast.notify(errorMessage(cause), "error");
    } finally {
      setBusy(false);
    }
  }

  if (files.forbidden) {
    return (
      <div className="memory-scope">
        <ScopeNotice what={t("memory.what")} onGranted={() => void files.reload()} />
      </div>
    );
  }

  return (
    <div className="memory-panes" data-section="memory-overview">
      <div className="memory-list-pane">
        <div className="memory-pane-head">
          {local === null ? null : local ? (
            <button type="button" className="memory-folder" disabled={busy} onClick={() => void reveal()}>
              <FolderOpen size={16} strokeWidth={ICON_STROKE} aria-hidden="true" />
              {t("memory.overview.openFolder")}
            </button>
          ) : (
            <button type="button" className="memory-folder" disabled={!list.length || busy} onClick={() => void download()}>
              <FolderDown size={16} strokeWidth={ICON_STROKE} aria-hidden="true" />
              {t("memory.overview.download")}
            </button>
          )}
        </div>
        <div className="memory-list-scroll">
          {files.error ? <ErrorNote>{files.error}</ErrorNote> : null}
          <ul className="memory-list" aria-label={t("memory.overview.files")}>
            {list.map((file) => (
              <li key={file.name}>
                <button
                  type="button"
                  className={cx("memory-list-row", file.name === open?.name && "memory-list-row--selected")}
                  aria-current={file.name === open?.name ? "true" : undefined}
                  onClick={() => setSelected(file.name)}
                >
                  <FileText {...FILE_ICON} />
                  <span className="memory-list-label">{file.name}</span>
                </button>
              </li>
            ))}
          </ul>
          {local === null ? null : <p className="memory-list-note">{t(local ? "memory.overview.folderNote" : "memory.overview.remoteNote")}</p>}
        </div>
      </div>
      {open ? <FilePane key={open.name} info={open} onSaved={() => void files.reload()} /> : <div className="memory-detail-pane" />}
    </div>
  );
}
