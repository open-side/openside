import { useId, type KeyboardEvent, type ReactNode } from "react";
import { useSearchParams } from "react-router";
import { t } from "../../i18n";
import { cx } from "../../ui";
import { rovingChoice } from "../../ui/misc";
import { McpsTab } from "./plugins/Mcps";
import type { TabFrame } from "./plugins/parts";
import { SkillsTab } from "./plugins/Skills";
import "./plugins.css";

type Tab = "skills" | "mcps";
const TABS: readonly Tab[] = ["skills", "mcps"];

/**
 * Settings › Plugins & MCPs: a Skills | MCPs tab strip under the page title. The tab lives in the
 * URL (`?tab=mcps`) so reload and Back return to it.
 */
export function PluginsSettings() {
  const [params, setParams] = useSearchParams();
  const tab: Tab = params.get("tab") === "mcps" ? "mcps" : "skills";
  const id = useId();

  function choose(next: Tab) {
    if (next === tab) return;
    setParams(
      (current) => {
        const updated = new URLSearchParams(current);
        if (next === "skills") updated.delete("tab");
        else updated.set("tab", next);
        return updated;
      },
      { preventScrollReset: true },
    );
  }

  // The tab renders the strip so its own actions (Import on Skills) share the row with it.
  const frame: TabFrame = {
    head: (actions: ReactNode) => (
      <div className="plugins-head">
        <div className="plugins-tabs" role="tablist" aria-label={t("plugins.tabs")} onKeyDown={(event: KeyboardEvent<HTMLDivElement>) => rovingChoice(event, TABS, TABS.indexOf(tab), ".plugins-tab", choose)}>
          {TABS.map((value) => (
            <button
              key={value}
              type="button"
              role="tab"
              id={`${id}-${value}`}
              aria-selected={value === tab}
              aria-controls={`${id}-panel`}
              tabIndex={value === tab ? 0 : -1}
              className={cx("plugins-tab", value === tab && "plugins-tab--selected")}
              onClick={() => choose(value)}
            >
              {value === "skills" ? t("plugins.tab.skills") : t("plugins.tab.mcps")}
            </button>
          ))}
        </div>
        {actions}
      </div>
    ),
    panel: { id: `${id}-panel`, role: "tabpanel", "aria-labelledby": `${id}-${tab}` },
  };

  return <div className="plugins-page">{tab === "skills" ? <SkillsTab frame={frame} /> : <McpsTab frame={frame} />}</div>;
}
