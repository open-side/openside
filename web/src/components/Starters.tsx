import { Globe, Table2, TextSearch, UserRoundCheck } from "lucide-react";
import { useEffect, useState } from "react";
import { useClient } from "../context";
import { ICON_STROKE } from "../ui";

/** `https://*.shop.example:8443` → `shop.example`; the host a person would type. */
export function siteHost(origin: string): string | null {
  try {
    const host = new URL(origin.replace("://*.", "://")).hostname.replace(/^www\./, "");
    return host || null;
  } catch {
    return null;
  }
}

const MAX_SITES = 4;

/**
 * What the agent is good at, as three prompts the person finishes, plus the sites the vault
 * already knows. A click only fills the composer; nothing is sent.
 */
export function Starters({ onPick }: { onPick: (text: string) => void }) {
  const client = useClient();
  const [sites, setSites] = useState<string[]>([]);

  useEffect(() => {
    let cancelled = false;
    // A locked vault, a token without the scope or an old daemon just means no site row.
    client
      .vaultItems()
      .then((list) => {
        if (cancelled || !list.available) return;
        const hosts = list.items.flatMap((item) => item.origins.map(siteHost).filter((host): host is string => host !== null));
        setSites([...new Set(hosts)].slice(0, MAX_SITES));
      })
      .catch(() => undefined);
    return () => {
      cancelled = true;
    };
  }, [client]);

  const site = sites[0];
  const starters = [
    {
      icon: UserRoundCheck,
      title: site ? `Check ${site} for me` : "Sign in and check something",
      text: site ? `Sign in to ${site} and tell me what’s new.` : "Sign in to ",
    },
    { icon: TextSearch, title: "Compare two pages", text: "Compare these two pages and tell me which is the better deal: " },
    { icon: Table2, title: "Fill in a form", text: "Fill in this form with my details, then let me review it before you submit: " },
  ];

  return (
    <div className="starters">
      <ul className="starter-list" aria-label="Ways to start">
        {starters.map(({ icon: Icon, title, text }) => (
          <li key={title}>
            <button type="button" className="starter" onClick={() => onPick(text)}>
              <Icon size={18} strokeWidth={ICON_STROKE} aria-hidden="true" />
              <span>{title}</span>
            </button>
          </li>
        ))}
      </ul>
      {sites.length > 0 ? (
        <div className="starter-sites" role="group" aria-label="Your saved sites">
          <span className="starter-sites-label">Your saved sites</span>
          {sites.map((host) => (
            <button key={host} type="button" className="chip-button" onClick={() => onPick(`Sign in to ${host} and tell me what’s new.`)}>
              <Globe size={14} strokeWidth={ICON_STROKE} aria-hidden="true" />
              {host}
            </button>
          ))}
        </div>
      ) : null}
    </div>
  );
}
