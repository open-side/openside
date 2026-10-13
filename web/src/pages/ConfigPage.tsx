import { useCallback, useMemo, useRef, useState } from "react";
import type { SettingsDocument } from "../api/types";
import { useClient } from "../context";
import { errorMessage, useResource } from "../hooks/useResource";
import {
  applyDrafts,
  DraftError,
  remoteEditable,
  schemaFields,
  toDraft,
  valueAt,
  type Draft,
  type Field,
} from "../settings/schemaForm";
import { Button, Segmented, Switch, useToast } from "../ui";
import { Badge, ErrorNote, ResourceView } from "./blocks";

function FieldInput({
  field,
  value,
  locked,
  onChange,
}: {
  field: Field;
  value: Draft;
  locked: boolean;
  onChange: (value: Draft) => void;
}) {
  const common = { id: `setting-${field.path}`, disabled: locked, "aria-describedby": `setting-${field.path}-help` };
  switch (field.kind) {
    case "boolean":
      return <Switch {...common} checked={value === true} onChange={onChange} />;
    case "integer":
    case "number":
      return (
        <input
          type="number"
          {...common}
          min={field.minimum}
          step={field.kind === "integer" ? 1 : "any"}
          value={String(value)}
          onChange={(e) => onChange(e.target.value)}
        />
      );
    case "enum":
      return (
        <select {...common} value={String(value)} onChange={(e) => onChange(e.target.value)}>
          {field.nullable ? <option value="">(not set)</option> : null}
          {field.options.map((option) => (
            <option key={option} value={option}>
              {option}
            </option>
          ))}
        </select>
      );
    case "list":
    case "json":
      return (
        <textarea
          {...common}
          rows={field.kind === "json" ? 4 : 2}
          value={String(value)}
          placeholder={field.kind === "list" ? "one per line" : undefined}
          onChange={(e) => onChange(e.target.value)}
        />
      );
    default:
      return (
        <input
          {...common}
          value={String(value)}
          placeholder={field.nullable ? "(not set)" : undefined}
          onChange={(e) => onChange(e.target.value)}
        />
      );
  }
}

type ConfigEditorProps = {
  document: SettingsDocument;
  onSave: (config: unknown) => Promise<SettingsDocument | null>;
  onEdit: () => void;
};

function ConfigForm({ document, onSave, onEdit }: ConfigEditorProps) {
  const toast = useToast();
  const fields = useMemo(() => schemaFields(document.schema, document.file_only), [document]);
  const [drafts, setDrafts] = useState<Record<string, Draft>>({});
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const sections = useMemo(() => [...new Set(fields.map((field) => field.section))], [fields]);
  const changed = Object.keys(drafts).length;

  async function save() {
    setBusy(true);
    setError(null);
    try {
      const next = await onSave(applyDrafts(document.config, fields, drafts));
      if (!next) return;
      setDrafts({});
      toast.notify("Settings saved.");
    } catch (cause) {
      setError(cause instanceof DraftError ? cause.message : errorMessage(cause));
    } finally {
      setBusy(false);
    }
  }

  return (
    <form
      className="config-form"
      aria-label="Configuration"
      onSubmit={(event) => {
        event.preventDefault();
        void save();
      }}
    >
      {sections.map((section) => (
        <fieldset key={section} className="config-group">
          <legend>{section}</legend>
          <div className="config-fields">
            {fields
              .filter((field) => field.section === section)
              .map((field) => {
                const locked = document.remote && !remoteEditable(field.path, document.remote_editable);
                const value = drafts[field.path] ?? toDraft(field, valueAt(document.config, field.path));
                const dot = field.path.indexOf(".");
                return (
                  <div key={field.path} className="config-field" data-key={field.path} data-kind={field.kind} data-locked={locked}>
                    <div className="config-field-text">
                      <label htmlFor={`setting-${field.path}`}>
                        <code className="config-key">
                          <span className="config-key-section">{field.path.slice(0, dot + 1)}</span>
                          {field.path.slice(dot + 1)}
                        </code>
                        {locked ? <Badge>local only</Badge> : null}
                        {drafts[field.path] !== undefined ? <Badge tone="attention">changed</Badge> : null}
                      </label>
                      <p id={`setting-${field.path}-help`}>{field.description}</p>
                    </div>
                    <div className="config-field-control">
                      <FieldInput
                        field={field}
                        value={value}
                        locked={locked}
                        onChange={(next) => {
                          onEdit();
                          setDrafts((current) => ({ ...current, [field.path]: next }));
                        }}
                      />
                    </div>
                  </div>
                );
              })}
          </div>
        </fieldset>
      ))}
      {error ? <ErrorNote>{error}</ErrorNote> : null}
      {changed ? (
        <div className="config-save">
          <span className="config-save-status" role="status">
            {changed} unsaved {changed === 1 ? "change" : "changes"}
          </span>
          <Button
            variant="ghost"
            onClick={() => {
              onEdit();
              setDrafts({});
            }}
          >
            Discard
          </Button>
          <Button type="submit" variant="primary" disabled={busy}>
            Save ({changed})
          </Button>
        </div>
      ) : null}
    </form>
  );
}

function ConfigJson({ document, onSave, onEdit }: ConfigEditorProps) {
  const toast = useToast();
  const [draft, setDraft] = useState<string | null>(null);
  const text = draft ?? JSON.stringify(document.config, null, 2);
  const [error, setError] = useState<string | null>(null);

  async function save() {
    let parsed: unknown;
    try {
      parsed = JSON.parse(text);
    } catch {
      setError("This is not valid JSON.");
      return;
    }
    try {
      const next = await onSave(parsed);
      if (!next) return;
      setError(null);
      setDraft(null);
      toast.notify("Settings saved.");
    } catch (cause) {
      setError(errorMessage(cause));
    }
  }

  return (
    <div className="dialog-form config-json">
      <label className="field">
        <span>Configuration document (JSON form of config.toml)</span>
        <textarea
          rows={20}
          value={text}
          spellCheck={false}
          onChange={(e) => {
            onEdit();
            setDraft(e.target.value);
          }}
        />
      </label>
      {error ? <ErrorNote>{error}</ErrorNote> : null}
      <div className="config-save">
        <Button variant="primary" onClick={() => void save()}>
          Save JSON
        </Button>
      </div>
    </div>
  );
}

/** Every configuration key, rendered from the daemon's own schema (`GET /v1/settings/config`). */
export function ConfigPage() {
  const client = useClient();
  const load = useCallback(() => client.settingsConfig(), [client]);
  const settings = useResource(load);
  const [saved, setSaved] = useState<SettingsDocument | null>(null);
  const [view, setView] = useState<"form" | "json">("form");
  const document = saved ?? settings.data;
  const generation = useRef(0);

  function edit() {
    generation.current += 1;
  }

  async function save(config: unknown): Promise<SettingsDocument | null> {
    const request = ++generation.current;
    try {
      const next = await client.updateSettingsConfig(config);
      // A response belongs to the draft that sent it, even after switching editors.
      if (request !== generation.current) return null;
      setSaved(next);
      return next;
    } catch (cause) {
      if (request !== generation.current) return null;
      throw cause;
    }
  }

  return (
    <div className="manage-body" data-section="config">
      <ResourceView resource={settings} what="settings">
        {() => null}
      </ResourceView>
      {!document || settings.forbidden ? null : (
        <>
          {document.remote ? (
            <p className="config-notice" role="note">
              You are connected as a paired device. Keys marked “local only” (commands, paths, listeners, policy, trust lists)
              can only be changed on the computer running openside.
            </p>
          ) : null}
          {document.restart_required ? (
            <p className="config-notice" role="status">
              Saved settings differ from the running daemon; they apply after the daemon restarts.
            </p>
          ) : null}
          <div className="config-toolbar">
            <Segmented
              kind="tab"
              label="Settings view"
              options={[
                { value: "form", label: "Form" },
                { value: "json", label: "JSON" },
              ]}
              value={view}
              onChange={setView}
            />
            {document.file_only.length ? (
              <p className="settings-text">Only in config.toml: {document.file_only.join(", ")}</p>
            ) : null}
          </div>
          {view === "form" ? (
            <ConfigForm document={document} onSave={save} onEdit={edit} />
          ) : (
            <ConfigJson document={document} onSave={save} onEdit={edit} />
          )}
        </>
      )}
    </div>
  );
}
