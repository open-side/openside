import { useId, type ReactNode } from "react";
import { InfoBanner } from "../../../ui";

/**
 * What the page hands each tab: `head` draws the tab strip with the tab's own actions at its end,
 * and `panel` goes on the element holding the tab's content (the ARIA tabpanel).
 */
export interface TabFrame {
  head: (actions: ReactNode) => ReactNode;
  panel: { id: string; role: "tabpanel"; "aria-labelledby": string };
}

/**
 * A labelled group ("Your skills 3", "Servers", "Presets"): the 14px muted label with an optional
 * lighter count, then one card whose `<li>` rows are split by hairlines. `bare` renders the
 * children (an `EmptyState` card, a banner) without the list card. `footnote` is a muted line under
 * the card, used to say which Aside rows openside has no engine for yet.
 */
export function PluginGroup({
  label,
  count,
  note,
  footnote,
  bare,
  children,
}: {
  label: string;
  count?: number;
  note?: string;
  footnote?: string;
  bare?: boolean;
  children: ReactNode;
}) {
  const id = useId();
  return (
    <section className="ui-section plugins-group" aria-labelledby={id}>
      <h2 className="ui-section-label plugins-group-label" id={id}>
        {label}
        {count !== undefined ? <span className="plugins-count">{count}</span> : null}
      </h2>
      {bare ? (
        children
      ) : (
        <div className="ui-card">
          {note ? <InfoBanner>{note}</InfoBanner> : null}
          <ul className="plugins-list" aria-label={label}>
            {children}
          </ul>
        </div>
      )}
      {footnote ? <p className="plugins-footnote">{footnote}</p> : null}
    </section>
  );
}

/** The rounded tile in front of a skill (30px) or an MCP server/preset (28px); no vendor logos. */
export function PluginIcon({ size, children }: { size: 28 | 30; children: ReactNode }) {
  return (
    <span className="plugins-icon" data-size={size} aria-hidden="true">
      {children}
    </span>
  );
}
