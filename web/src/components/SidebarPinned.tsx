import { Globe } from "lucide-react";
import { useEffect, useState } from "react";
import type { PinnedTab } from "../api/types";
import { useClient } from "../context";
import { originOf, useFavicon } from "../hooks/useFavicon";
import { ICON_STROKE } from "../ui";

const MAX_TILES = 6;

/**
 * The synced pinned tabs as favicon tiles. They open in a new browser tab: openside runs inside
 * the person's own browser. Nothing renders while empty, unreadable or forbidden for this token.
 */
export function SidebarPinned() {
  const client = useClient();
  const [tabs, setTabs] = useState<PinnedTab[]>([]);
  useEffect(() => {
    let live = true;
    client
      .pinnedTabs()
      .then((list) => list.tabs.slice(0, MAX_TILES))
      .then(
        (first) => live && setTabs(first),
        () => live && setTabs([]),
      );
    return () => {
      live = false;
    };
  }, [client]);
  if (tabs.length === 0) return null;
  return (
    <section className="sidebar-pinned" aria-label="Pinned">
      <ul>
        {tabs.map((tab) => (
          <li key={tab.id}>
            <PinnedTile tab={tab} />
          </li>
        ))}
      </ul>
    </section>
  );
}

function PinnedTile({ tab }: { tab: PinnedTab }) {
  const origin = originOf(tab.url);
  const icon = useFavicon(origin);
  const label = tab.title?.trim() || (origin ? new URL(origin).host : tab.url);
  return (
    <a className="sidebar-pinned-tile" href={tab.url} target="_blank" rel="noopener noreferrer" title={label} aria-label={label}>
      {icon ? <img src={icon} alt="" width={18} height={18} /> : <Globe size={18} strokeWidth={ICON_STROKE} aria-hidden="true" />}
    </a>
  );
}
