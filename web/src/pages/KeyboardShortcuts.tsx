import { keyLabel, shortcutTable } from "../shortcuts";
import "../shortcuts.css";
import { StatusRow } from "./blocks";

/** The Settings table of keys openside handles; nothing the app does not really bind is listed. */
export function KeyboardShortcuts() {
  return (
    <section aria-label="Keyboard shortcuts">
      <h3 className="settings-heading">Keyboard shortcuts</h3>
      <div className="settings-group">
        {shortcutTable().map(({ keys, action }) => (
          <StatusRow key={action} label={action}>
            <kbd className="key-chip">{keyLabel(keys)}</kbd>
          </StatusRow>
        ))}
      </div>
    </section>
  );
}
