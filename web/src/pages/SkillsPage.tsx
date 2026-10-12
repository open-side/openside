import { BookOpen } from "lucide-react";
import { useCallback, useState, type FormEvent } from "react";
import type { SkillDetail, SkillSummary } from "../api/types";
import { useClient } from "../context";
import { errorMessage, useResource } from "../hooks/useResource";
import { Button, Dialog, DialogActions, useToast } from "../ui";
import {
  AddButton,
  CardActions,
  EmptyState,
  ErrorNote,
  Loading,
  ResourceCard,
  SectionList,
  useConfirm,
  useDialogState,
} from "./blocks";

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
    <form className="dialog-form" onSubmit={save} aria-label={`Edit ${name}`}>
      <label className="field">
        <span>Description</span>
        <input value={description ?? skill.data.description} onChange={(e) => setDescription(e.target.value)} />
      </label>
      <label className="field">
        <span>Instructions (Markdown)</span>
        <textarea rows={10} value={body ?? skill.data.body} onChange={(e) => setBody(e.target.value)} />
      </label>
      {skill.data.resources.length ? (
        <p className="settings-text">Bundled files: {skill.data.resources.map((r) => r.path).join(", ")}</p>
      ) : null}
      {skill.error ? <ErrorNote>{skill.error}</ErrorNote> : null}
      <DialogActions>
        <Button onClick={onCancel}>Cancel</Button>
        <Button type="submit" variant="primary" disabled={busy}>
          Save
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
    <form className="dialog-form" onSubmit={submit} aria-label="New skill">
      <label className="field">
        <span>Name</span>
        <input value={name} onChange={(e) => setName(e.target.value)} placeholder="lowercase-with-hyphens" required />
      </label>
      <label className="field">
        <span>Description</span>
        <input value={description} onChange={(e) => setDescription(e.target.value)} required />
      </label>
      <label className="field">
        <span>Instructions (Markdown)</span>
        <textarea rows={6} value={body} onChange={(e) => setBody(e.target.value)} />
      </label>
      {error ? <ErrorNote>{error}</ErrorNote> : null}
      <DialogActions>
        <Button onClick={onCancel}>Cancel</Button>
        <Button type="submit" variant="primary" disabled={busy || !name.trim() || !description.trim()}>
          Create
        </Button>
      </DialogActions>
    </form>
  );
}

/** Skill library: list, create, edit, enable/disable and delete user skills. */
export function SkillsPage() {
  const client = useClient();
  const toast = useToast();
  const load = useCallback(() => client.listSkills(), [client]);
  const list = useResource(load);
  const [creating, setCreating] = useState(false);
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

  function card(skill: SkillSummary) {
    const editable = skill.source === "user";
    return (
      <li key={`${skill.source}:${skill.name}`}>
        <ResourceCard
          data-skill={skill.name}
          title={`/${skill.name}`}
          detail={editable ? "user" : `${skill.source} · read-only`}
          badge={skill.enabled ? "Enabled" : "Disabled"}
          badgeTone={skill.enabled ? "accent" : "muted"}
        >
          {skill.description && <p className="resource-card-body">{skill.description}</p>}
          {editable && (
            <CardActions>
              <Button size="compact" onClick={() => editor.show(skill.name)}>
                Edit
              </Button>
              <Button
                size="compact"
                onClick={() => void act(() => client.updateSkill(skill.name, { enabled: !skill.enabled }))}
              >
                {skill.enabled ? "Disable" : "Enable"}
              </Button>
              <Button
                size="compact"
                variant="danger"
                onClick={() =>
                  ask({
                    title: "Delete skill?",
                    description: `/${skill.name} and its bundled files will be removed.`,
                    confirmLabel: "Delete",
                    onConfirm: () => void act(() => client.deleteSkill(skill.name), "Skill deleted."),
                  })
                }
              >
                Delete
              </Button>
            </CardActions>
          )}
        </ResourceCard>
      </li>
    );
  }

  return (
    <div className="manage-body" data-section="skills">
      <SectionList
        label="Skill library"
        actions={
<AddButton onClick={() => setCreating(true)}>New skill</AddButton>
        }
      >
        {list.error ? <ErrorNote>{list.error}</ErrorNote> : null}
        {!list.data ? (
          list.error ? null : <Loading />
        ) : list.data.skills.length === 0 ? (
          <EmptyState icon={BookOpen} title="No skills yet" detail="Skills add reusable instructions you can call with /name." />
        ) : (
          <ul className="resource-list" aria-label="Skills">
            {list.data.skills.map(card)}
          </ul>
        )}
      </SectionList>
      {list.data?.problems.length ? (
        <SectionList title="Rejected skill folders" label="Skill problems">
          <ul className="resource-list">
            {list.data.problems.map((problem) => (
              <li key={`${problem.source}:${problem.name}`}>
                <ResourceCard title={problem.name} detail={problem.source} badge="Rejected" badgeTone="attention">
                  <p className="resource-card-body error-note">{problem.error}</p>
                </ResourceCard>
              </li>
            ))}
          </ul>
        </SectionList>
      ) : null}

      <Dialog
        open={creating}
        onClose={() => setCreating(false)}
        title="New skill"
        description="Skills add reusable instructions the agent loads when you call /name."
      >
        <NewSkillForm
          onCancel={() => setCreating(false)}
          onCreated={(name) => {
            setCreating(false);
            toast.notify(`/${name} created.`);
            void list.reload();
          }}
        />
      </Dialog>
      <Dialog open={editor.open} onClose={editor.close} title="Edit skill" description={editor.value ? `/${editor.value}` : undefined}>
        {editor.value && (
          <SkillEditor
            key={editor.value}
            name={editor.value}
            onCancel={editor.close}
            onDone={() => {
              editor.close();
              toast.notify("Skill saved.");
              void list.reload();
            }}
          />
        )}
      </Dialog>
      {confirm}
    </div>
  );
}
