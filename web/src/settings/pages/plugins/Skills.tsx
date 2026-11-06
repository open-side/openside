import { BookOpen, ChevronDown, Copy, FolderGit2, FolderInput, Pencil, Plus, Sparkles, Trash2, TriangleAlert } from "lucide-react";
import { useCallback, useState, type FormEvent, type ReactNode } from "react";
import type { SkillDetail, SkillList, SkillSummary } from "../../../api/types";
import { useClient } from "../../../context";
import { errorMessage, useResource } from "../../../hooks/useResource";
import { t } from "../../../i18n";
import { ErrorNote, Loading, ResourceView, RowMenu, useConfirm, useDialogState } from "../../../pages/blocks";
import { Button, copyText, Dialog, DialogActions, EmptyState, ICON_STROKE, MenuItem, PopoverMenu, Switch, useToast } from "../../../ui";
import { PluginGroup, PluginIcon, type TabFrame } from "./parts";

const MENU_ICON = { size: 18, strokeWidth: ICON_STROKE } as const;

function SkillEditor({ name, onDone, onCancel }: { name: string; onDone: () => void; onCancel: () => void }) {
  const client = useClient();
  const load = useCallback(() => client.getSkill(name), [client, name]);
  const skill = useResource<SkillDetail>(load);
  const [description, setDescription] = useState<string | null>(null);
  const [body, setBody] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  async function save(event: FormEvent) {
    event.preventDefault();
    if (!skill.data) return;
    setBusy(true);
    try {
      await client.updateSkill(name, {
        description: description ?? skill.data.description,
        body: body ?? skill.data.body,
      });
      onDone();
    } catch (cause) {
      skill.setError(errorMessage(cause));
    } finally {
      setBusy(false);
    }
  }

  if (!skill.data) return skill.error ? <ErrorNote>{skill.error}</ErrorNote> : <Loading />;
  return (
    <form className="dialog-form" onSubmit={save} aria-label={t("plugins.skills.editLabel", { name })}>
      <label className="field">
        <span>{t("plugins.skills.description")}</span>
        <input value={description ?? skill.data.description} onChange={(e) => setDescription(e.target.value)} />
      </label>
      <label className="field">
        <span>{t("plugins.skills.instructions")}</span>
        <textarea rows={10} value={body ?? skill.data.body} onChange={(e) => setBody(e.target.value)} />
      </label>
      {skill.data.resources.length ? (
        <p className="plugins-hint">{t("plugins.skills.bundled", { files: skill.data.resources.map((r) => r.path).join(", ") })}</p>
      ) : null}
      {skill.error ? <ErrorNote>{skill.error}</ErrorNote> : null}
      <DialogActions>
        <Button variant="ghost" onClick={onCancel}>
          {t("common.cancel")}
        </Button>
        <Button type="submit" variant="primary" disabled={busy}>
          {t("common.save")}
        </Button>
      </DialogActions>
    </form>
  );
}

function NewSkillForm({ onCreated, onCancel }: { onCreated: (name: string) => void; onCancel: () => void }) {
  const client = useClient();
  const [name, setName] = useState("");
  const [description, setDescription] = useState("");
  const [body, setBody] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  async function submit(event: FormEvent) {
    event.preventDefault();
    setBusy(true);
    setError(null);
    try {
      const created = name.trim();
      await client.createSkill({ name: created, description: description.trim(), body, resources: [] });
      onCreated(created);
    } catch (cause) {
      setError(errorMessage(cause));
    } finally {
      setBusy(false);
    }
  }

  return (
    <form className="dialog-form" onSubmit={submit} aria-label={t("plugins.import.newSkill")}>
      <label className="field">
        <span>{t("plugins.skills.name")}</span>
        <input value={name} onChange={(e) => setName(e.target.value)} placeholder={t("plugins.skills.namePlaceholder")} required />
      </label>
      <label className="field">
        <span>{t("plugins.skills.description")}</span>
        <input value={description} onChange={(e) => setDescription(e.target.value)} required />
      </label>
      <label className="field">
        <span>{t("plugins.skills.instructions")}</span>
        <textarea rows={6} value={body} onChange={(e) => setBody(e.target.value)} />
      </label>
      {error ? <ErrorNote>{error}</ErrorNote> : null}
      <DialogActions>
        <Button variant="ghost" onClick={onCancel}>
          {t("common.cancel")}
        </Button>
        <Button type="submit" variant="primary" disabled={busy || !name.trim() || !description.trim()}>
          {t("common.create")}
        </Button>
      </DialogActions>
    </form>
  );
}

function CommandLine({ command }: { command: string }) {
  const toast = useToast();
  return (
    <div className="plugins-command">
      <code>{command}</code>
      <Button
        size="small"
        aria-label={t("plugins.folder.copy")}
        title={t("plugins.folder.copy")}
        onClick={() => void copyText(command).then((copied) => copied && toast.notify(t("plugins.folder.copied")))}
      >
        <Copy size={12} strokeWidth={ICON_STROKE} aria-hidden="true" />
      </Button>
    </div>
  );
}

/**
 * The daemon has no endpoint that reads an arbitrary folder (only the CLI's `skills import`
 * copies one into the library, docs/specs/skills.md §3), so "From a folder" explains that path
 * instead of pretending to import.
 */
function FolderImportDialog({ open, onClose }: { open: boolean; onClose: () => void }) {
  return (
    <Dialog open={open} onClose={onClose} title={t("plugins.folder.title")} description={t("plugins.folder.description")}>
      <div className="plugins-folder">
        <p>{t("plugins.folder.step")}</p>
        <CommandLine command="openside skills import <folder>" />
        <p>{t("plugins.folder.agent")}</p>
        <CommandLine command="openside skills import <name> --from claude" />
        <p className="plugins-hint">{t("plugins.folder.note")}</p>
      </div>
      <DialogActions>
        <Button variant="primary" onClick={onClose}>
          {t("common.done")}
        </Button>
      </DialogActions>
    </Dialog>
  );
}

/** Aside's black split button: "+ Import" and a ⌄ with every way to add a skill. */
function ImportButton({ onNew, onFolder }: { onNew: () => void; onFolder: () => void }) {
  return (
    <div className="plugins-split" role="group" aria-label={t("plugins.import")}>
      <Button variant="primary" className="plugins-split-main" onClick={onFolder}>
        <Plus size={14} strokeWidth={ICON_STROKE} aria-hidden="true" />
        {t("plugins.import")}
      </Button>
      <PopoverMenu
        label={t("plugins.import.more")}
        trigger={
          <Button variant="primary" className="plugins-split-more" aria-label={t("plugins.import.more")} title={t("plugins.import.more")}>
            <ChevronDown size={14} strokeWidth={ICON_STROKE} aria-hidden="true" />
          </Button>
        }
      >
        <MenuItem icon={<Sparkles {...MENU_ICON} />} onClick={onNew}>
          {t("plugins.import.newSkill")}
        </MenuItem>
        <MenuItem icon={<FolderInput {...MENU_ICON} />} onClick={onFolder}>
          {t("plugins.import.folder")}
        </MenuItem>
      </PopoverMenu>
    </div>
  );
}

/** A 56px skill row: icon tile, name, one truncated line of description, then the controls. */
function SkillRow({ skill, onOpen, controls }: { skill: SkillSummary; onOpen?: () => void; controls?: ReactNode }) {
  const copy = (
    <>
      <PluginIcon size={30}>{skill.source === "user" ? <BookOpen /> : <FolderGit2 />}</PluginIcon>
      <span className="ui-row-copy">
        <span className="ui-row-title">{skill.name}</span>
        {skill.description ? <span className="ui-row-description plugins-truncate">{skill.description}</span> : null}
      </span>
    </>
  );
  return (
    <li className="ui-row plugins-row" data-skill={skill.name}>
      {onOpen ? (
        <button type="button" className="plugins-row-open" aria-label={t("plugins.skills.open", { name: skill.name })} onClick={onOpen}>
          {copy}
        </button>
      ) : (
        <span className="plugins-row-open">{copy}</span>
      )}
      {controls ? <div className="ui-row-control">{controls}</div> : null}
    </li>
  );
}

/** Settings › Plugins & MCPs › Skills: the library grouped like Aside, with create/edit/toggle/delete. */
export function SkillsTab({ frame }: { frame: TabFrame }) {
  const client = useClient();
  const toast = useToast();
  const load = useCallback(() => client.listSkills(), [client]);
  const list = useResource<SkillList>(load);
  const [creating, setCreating] = useState(false);
  const [importing, setImporting] = useState(false);
  const editor = useDialogState<string>();
  const [confirm, ask] = useConfirm();

  async function act(action: () => Promise<unknown>, done?: string) {
    try {
      await action();
      await list.reload();
      if (done) toast.notify(done);
    } catch (cause) {
      list.setError(errorMessage(cause));
    }
  }

  function userControls(skill: SkillSummary) {
    return (
      <>
        <Switch
          label={t("plugins.skills.enable", { name: skill.name })}
          checked={skill.enabled}
          onChange={(enabled) => void act(() => client.updateSkill(skill.name, { enabled }))}
        />
        <RowMenu name={`/${skill.name}`}>
          <MenuItem icon={<Pencil {...MENU_ICON} />} onClick={() => editor.show(skill.name)}>
            {t("common.edit")}
          </MenuItem>
          <MenuItem
            danger
            icon={<Trash2 {...MENU_ICON} />}
            onClick={() =>
              ask({
                title: t("plugins.skills.deleteTitle"),
                description: t("plugins.skills.deleteDescription", { name: skill.name }),
                confirmLabel: t("common.delete"),
                onConfirm: () => void act(() => client.deleteSkill(skill.name), t("plugins.skills.deleted")),
              })
            }
          >
            {t("common.delete")}
          </MenuItem>
        </RowMenu>
      </>
    );
  }

  function body(data: SkillList) {
    const project = data.skills.filter((skill) => skill.source === "project");
    const yours = data.skills.filter((skill) => skill.source === "user");
    return (
      <>
        {project.length ? (
          <PluginGroup label={t("plugins.skills.project")} count={project.length}>
            {project.map((skill) => (
              <SkillRow key={`project:${skill.name}`} skill={skill} controls={<span className="plugins-muted">{t("plugins.skills.readOnly")}</span>} />
            ))}
          </PluginGroup>
        ) : null}
        {yours.length ? (
          <PluginGroup label={t("plugins.skills.yours")} count={yours.length} footnote={t("plugins.skills.missingNote")}>
            {yours.map((skill) => (
              <SkillRow key={`user:${skill.name}`} skill={skill} onOpen={() => editor.show(skill.name)} controls={userControls(skill)} />
            ))}
          </PluginGroup>
        ) : (
          <PluginGroup label={t("plugins.skills.yours")} footnote={t("plugins.skills.missingNote")} bare>
            <EmptyState
              icon={<BookOpen size={16} strokeWidth={ICON_STROKE} />}
              title={t("plugins.skills.emptyTitle")}
              description={t("plugins.skills.emptyDescription")}
              action={<Button onClick={() => setCreating(true)}>{t("plugins.import.newSkill")}</Button>}
            />
          </PluginGroup>
        )}
        {data.problems.length ? (
          <PluginGroup label={t("plugins.skills.problems")} count={data.problems.length} note={t("plugins.skills.problemsNote")}>
            {data.problems.map((problem) => (
              <li key={`${problem.source}:${problem.name}`} className="ui-row plugins-row plugins-row--muted">
                <span className="plugins-row-open">
                  <PluginIcon size={30}>
                    <TriangleAlert />
                  </PluginIcon>
                  <span className="ui-row-copy">
                    <span className="ui-row-title">{problem.name}</span>
                    <span className="ui-row-description plugins-truncate">{problem.error}</span>
                  </span>
                </span>
              </li>
            ))}
          </PluginGroup>
        ) : null}
      </>
    );
  }

  return (
    <>
      {frame.head(list.forbidden ? null : <ImportButton onNew={() => setCreating(true)} onFolder={() => setImporting(true)} />)}
      <div className="plugins-groups" {...frame.panel}>
        <ResourceView resource={list} what={t("plugins.skills.what")}>
          {body}
        </ResourceView>
      </div>

      <Dialog open={creating} onClose={() => setCreating(false)} title={t("plugins.import.newSkill")} description={t("plugins.skills.newDescription")}>
        <NewSkillForm
          onCancel={() => setCreating(false)}
          onCreated={(name) => {
            setCreating(false);
            toast.notify(t("plugins.skills.created", { name }));
            void list.reload();
          }}
        />
      </Dialog>
      <Dialog open={editor.open} onClose={editor.close} title={t("plugins.skills.editTitle")} description={editor.value ? `/${editor.value}` : undefined}>
        {editor.value && (
          <SkillEditor
            key={editor.value}
            name={editor.value}
            onCancel={editor.close}
            onDone={() => {
              editor.close();
              toast.notify(t("plugins.skills.saved"));
              void list.reload();
            }}
          />
        )}
      </Dialog>
      <FolderImportDialog open={importing} onClose={() => setImporting(false)} />
      {confirm}
    </>
  );
}
