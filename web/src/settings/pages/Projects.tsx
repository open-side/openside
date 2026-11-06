import { Plus } from "lucide-react";
import { useCallback, useEffect, useId, useState, type FormEvent } from "react";
import { ApiError, apiErrorMessage } from "../../api/client";
import type { Project } from "../../api/types";
import { folderName } from "../../components/ComposerOptions";
import { useClient } from "../../context";
import { useResource } from "../../hooks/useResource";
import { t } from "../../i18n";
import { ErrorNote, ResourceView, RowMenu, useConfirm, useDialogState } from "../../pages/blocks";
import { Badge, Button, Dialog, DialogActions, ICON_STROKE, MenuItem, SettingsCard, SettingsRow, SettingsSection } from "../../ui";
import "./agents.css";
import "./projects.css";

// The daemon's limits (openside-proto MAX_PROJECT_NAME_CHARS / MAX_PROJECT_INSTRUCTIONS_BYTES).
const MAX_NAME_CHARS = 80;
const MAX_INSTRUCTIONS_BYTES = 16 * 1024;

function failure(cause: unknown): string {
  if (cause instanceof ApiError) return apiErrorMessage(cause.code) ?? cause.message;
  return t("browser.requestFailed");
}

/** `null` creates a project; a project edits its name and instructions (its folder is fixed). */
type Editing = { project: Project | null };

/**
 * Create or edit one project. Creating names a folder (typed, or chosen in the picker of the
 * computer running openside) or leaves it empty to start from scratch in a new empty folder.
 */
function ProjectDialog({ open, editing, local, onClose, onSaved }: { open: boolean; editing: Editing | null; local: boolean; onClose: () => void; onSaved: () => void }) {
  const client = useClient();
  const project = editing?.project ?? null;
  const [name, setName] = useState("");
  const [root, setRoot] = useState("");
  const [instructions, setInstructions] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const nameId = useId();
  const rootId = useId();
  const rootHint = useId();
  const instructionsId = useId();
  const instructionsHint = useId();

  useEffect(() => {
    if (!open) return;
    setName(project?.name ?? "");
    setRoot("");
    setInstructions(project?.instructions ?? "");
    setError(null);
  }, [open, project]);

  async function choose() {
    setError(null);
    try {
      const { path } = await client.pickFolder();
      if (!path) return;
      setRoot(path);
      setName((current) => current || folderName(path));
    } catch (cause) {
      setError(failure(cause));
    }
  }

  async function submit(event: FormEvent) {
    event.preventDefault();
    if (new TextEncoder().encode(instructions).length > MAX_INSTRUCTIONS_BYTES) {
      setError(apiErrorMessage("instructions_too_large") ?? null);
      return;
    }
    setBusy(true);
    setError(null);
    try {
      if (project) {
        await client.updateProject(project.id, { name: name.trim(), instructions });
      } else {
        await client.createProject({ name: name.trim(), ...(root.trim() ? { root: root.trim() } : {}), ...(instructions.trim() ? { instructions } : {}) });
      }
      onSaved();
      onClose();
    } catch (cause) {
      setError(failure(cause));
    } finally {
      setBusy(false);
    }
  }

  return (
    <Dialog
      open={open}
      onClose={onClose}
      title={t(project ? "agentSettings.projects.dialog.edit" : "agentSettings.projects.dialog.create")}
      description={t(project ? "agentSettings.projects.dialog.editDescription" : "agentSettings.projects.dialog.createDescription")}
    >
      <form className="projects-form" onSubmit={(event) => void submit(event)}>
        <label htmlFor={nameId}>
          {t("agentSettings.projects.name")}
          <input id={nameId} value={name} maxLength={MAX_NAME_CHARS} autoComplete="off" onChange={(event) => setName(event.target.value)} />
        </label>
        {project ? null : (
          <div className="projects-field">
            <label htmlFor={rootId}>{t("agentSettings.projects.folder")}</label>
            <div className="projects-folder-row">
              <input
                id={rootId}
                className="projects-mono"
                value={root}
                placeholder="/home/me/code"
                spellCheck={false}
                autoCapitalize="none"
                aria-describedby={rootHint}
                onChange={(event) => setRoot(event.target.value)}
              />
              <Button type="button" disabled={!local || busy} onClick={() => void choose()}>
                {t("agentSettings.projects.choose")}
              </Button>
            </div>
            <small id={rootHint}>{t("agentSettings.projects.folderHint")}</small>
          </div>
        )}
        <div className="projects-field">
          <label htmlFor={instructionsId}>{t("agentSettings.projects.instructions")}</label>
          <textarea id={instructionsId} value={instructions} rows={6} aria-describedby={instructionsHint} onChange={(event) => setInstructions(event.target.value)} />
          <small id={instructionsHint}>{t("agentSettings.projects.instructionsHint")}</small>
        </div>
        {error ? <ErrorNote>{error}</ErrorNote> : null}
        <DialogActions>
          <Button variant="ghost" onClick={onClose}>
            {t("common.cancel")}
          </Button>
          <Button type="submit" variant="primary" disabled={busy || !name.trim()}>
            {busy ? t("common.saving") : t(project ? "common.save" : "common.create")}
          </Button>
        </DialogActions>
      </form>
    </Dialog>
  );
}

/**
 * Settings › Projects (logged-in/47, vm2/projects-0): Aside's single "+ Create" card, then the
 * projects under it, each with Edit and Delete. A project is made on the computer running
 * openside (its folder lives there); other devices list, edit and delete them. A project whose
 * folder has no recorded identity is badged; only the computer running openside confirms it.
 */
export function ProjectsSettings() {
  const client = useClient();
  const load = useCallback(() => client.listProjects(), [client]);
  const projects = useResource(load);
  const dialog = useDialogState<Editing>();
  const [confirm, ask] = useConfirm();
  // The settings document's `remote` is the daemon's verdict on this browser's local authority;
  // null until it answers, so Create does not switch under the pointer.
  const [local, setLocal] = useState<boolean | null>(null);

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

  async function remove(project: Project) {
    try {
      await client.deleteProject(project.id);
      projects.setError(null);
    } catch (cause) {
      projects.setError(failure(cause));
    }
    await projects.reload();
  }

  async function confirmFolder(project: Project) {
    try {
      await client.updateProject(project.id, { confirm_root: true });
      projects.setError(null);
    } catch (cause) {
      projects.setError(failure(cause));
    }
    await projects.reload();
  }

  return (
    <div className="agents-page">
      <SettingsSection>
        <SettingsCard>
          <button
            type="button"
            className="projects-create"
            disabled={local !== true}
            aria-describedby={local === false ? "projects-remote" : undefined}
            onClick={() => dialog.show({ project: null })}
          >
            <Plus strokeWidth={ICON_STROKE} aria-hidden="true" />
            {t("agentSettings.projects.create")}
          </button>
        </SettingsCard>
        {local === false ? (
          <p className="agents-note" id="projects-remote">
            {t("agentSettings.projects.remote")}
          </p>
        ) : null}
        <ResourceView resource={projects} what={t("agentSettings.projects.list")}>
          {(data) =>
            data.projects.length ? (
              <SettingsCard className="projects-list">
                <ul aria-label={t("agentSettings.projects.list")}>
                  {data.projects.map((project) => (
                    <li key={project.id}>
                      <SettingsRow
                        title={project.name}
                        badge={project.needs_confirmation ? <Badge>{t("agentSettings.projects.needsConfirming")}</Badge> : undefined}
                        description={
                          <>
                            <span className="projects-mono">{project.root}</span>
                            {project.needs_confirmation && local !== null ? (
                              <span className="projects-confirm-note">{t(local ? "agentSettings.projects.confirmNote" : "agentSettings.projects.confirmRemote")}</span>
                            ) : null}
                          </>
                        }
                        control={
                          <span className="projects-row-actions">
                            {project.needs_confirmation && local === true ? (
                              <Button size="small" onClick={() => void confirmFolder(project)}>
                                {t("agentSettings.projects.confirm")}
                              </Button>
                            ) : null}
                            <RowMenu name={project.name}>
                              <MenuItem onClick={() => dialog.show({ project })}>{t("common.edit")}</MenuItem>
                              <MenuItem
                                danger
                                onClick={() =>
                                  ask({
                                    title: t("agentSettings.projects.delete.title", { name: project.name }),
                                    description: t("agentSettings.projects.delete.description"),
                                    confirmLabel: t("common.delete"),
                                    onConfirm: () => void remove(project),
                                  })
                                }
                              >
                                {t("common.delete")}
                              </MenuItem>
                            </RowMenu>
                          </span>
                        }
                      />
                    </li>
                  ))}
                </ul>
              </SettingsCard>
            ) : null
          }
        </ResourceView>
      </SettingsSection>
      <ProjectDialog open={dialog.open} editing={dialog.value} local={local === true} onClose={dialog.close} onSaved={() => void projects.reload()} />
      {confirm}
    </div>
  );
}
