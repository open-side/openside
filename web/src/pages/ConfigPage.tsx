import { useCallback, useMemo, useState } from "react";
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
import { Button, Segmented, useToast } from "../ui";
import { ErrorNote, Loading } from "./blocks";

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
      return <input type="checkbox" role="switch" {...common} checked={value === true} onChange={(e) => onChange(e.target.checked)} />;
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

function ConfigForm({ document, onSaved }: { document: SettingsDocument; onSaved: (next: SettingsDocument) => void }) {
  const client = useClient();
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
      onSaved(await client.updateSettingsConfig(applyDrafts(document.config, fields, drafts)));
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
          {fields
            .filter((field) => field.section === section)
            .map((field) => {
              const locked = document.remote && !remoteEditable(field.path, document.remote_editable);
              const value = drafts[field.path] ?? toDraft(field, valueAt(document.config, field.path));
              return (
                <div key={field.path} className="config-field" data-key={field.path} data-locked={locked}>
                  <label htmlFor={`setting-${field.path}`}>
                    <code>{field.path}</code>
                    {locked ? <span className="badge badge--muted">local only</span> : null}
                    {drafts[field.path] !== undefined ? <span className="badge badge--accent">changed</span> : null}
                  </label>
                  <FieldInput
                    field={field}
                    value={value}
                    locked={locked}
                    onChange={(next) => setDrafts((current) => ({ ...current, [field.path]: next }))}
                  />
                  <p id={`setting-${field.path}-help`}>{field.description}</p>
                </div>
              );
            })}
        </fieldset>
      ))}
      {error ? <ErrorNote>{error}</ErrorNote> : null}
      <div className="config-save">
        <Button type="submit" variant="primary" disabled={busy || changed === 0}>
          Save {changed ? `(${changed})` : ""}
        </Button>
        <Button variant="ghost" disabled={changed === 0} onClick={() => setDrafts({})}>
          Discard
        </Button>
      </div>
    </form>
  );
}

function ConfigJson({ document, onSaved }: { document: SettingsDocument; onSaved: (next: SettingsDocument) => void }) {
  const client = useClient();
  const toast = useToast();
  const [text, setText] = useState(() => JSON.stringify(document.config, null, 2));
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
      const next = await client.updateSettingsConfig(parsed);
      setError(null);
      setText(JSON.stringify(next.config, null, 2));
      onSaved(next);
      toast.notify("Settings saved.");
    } catch (cause) {
      setError(errorMessage(cause));
    }
  }

  return (
    <div className="dialog-form config-json">
      <label className="field">
        <span>Configuration document (JSON form of config.toml)</span>
        <textarea rows={20} value={text} spellCheck={false} onChange={(e) => setText(e.target.value)} />
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

  return (
    <div className="manage-body" data-section="config">
      {settings.error ? <ErrorNote>{settings.error}</ErrorNote> : null}
      {!document ? (
        settings.error ? null : <Loading />
      ) : (
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
          {document.file_only.length ? (
            <p className="settings-text">Edited only in config.toml: {document.file_only.join(", ")}.</p>
          ) : null}
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
          {view === "form" ? (
            <ConfigForm key={JSON.stringify(document.config)} document={document} onSaved={setSaved} />
          ) : (
            <ConfigJson key={JSON.stringify(document.config)} document={document} onSaved={setSaved} />
          )}
        </>
      )}
    </div>
  );
}
