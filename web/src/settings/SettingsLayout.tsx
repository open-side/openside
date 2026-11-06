import { ArrowUpRight, ChevronLeft, CircleHelp, Search } from "lucide-react";
import { useEffect, useRef, useState, useSyncExternalStore, type KeyboardEvent } from "react";
import { Link, Navigate, useLocation, useNavigate, useNavigationType } from "react-router";
import { AppHeader } from "../components/AppHeader";
import { useClient } from "../context";
import { openInBrowser } from "../hooks/usePersonBrowser";
import { t, type MessageKey } from "../i18n";
import { PHONE } from "../shortcuts";
import { Badge, cx, ICON_STROKE, useToast } from "../ui";
import {
  isMemoryPage,
  SETTINGS_FOOTER,
  SETTINGS_NAV,
  settingsPageOf,
  settingsPath,
  settingsTitle,
  type SettingsFooterLink,
  type SettingsNavPage,
  type SettingsPageId,
} from "./nav";
import { SETTINGS_BODY } from "./pages";
import "../pages/pages.css";
import "../pages/signin.css";
import "./settings.css";

function subscribePhone(onChange: () => void) {
  if (typeof window.matchMedia !== "function") return () => undefined;
  const query = window.matchMedia(PHONE);
  query.addEventListener("change", onChange);
  return () => query.removeEventListener("change", onChange);
}

const phoneNow = () => typeof window.matchMedia === "function" && window.matchMedia(PHONE).matches;

/** Memory › Overview and History are two panes filling the sheet; their title is for screen readers only. */
const FULL_BLEED: Partial<Record<SettingsPageId, true>> = { memory: true, "memory/history": true };
/**
 * Pages whose title carries Aside's ⓘ: Memory › Configure is titled "Configure memories ⓘ",
 * longer than its nav label; Archived chats keeps its label.
 */
const HEADING: Partial<Record<SettingsPageId, { title: MessageKey; help: MessageKey }>> = {
  "memory/configure": { title: "memory.configure.title", help: "memory.configure.help" },
  archived: { title: "settings.nav.archived.title", help: "settings.archived.help" },
};

function matches(item: SettingsNavPage, needle: string): boolean {
  return `${t(item.title)} ${t(item.keywords)}`.toLowerCase().includes(needle);
}

interface ShownPage {
  item: SettingsNavPage;
  children?: readonly SettingsNavPage[];
}

/** The tree as shown: filtered by the search, Memory open on its pages or when a child matches. */
function shownGroups(current: SettingsPageId | null, needle: string) {
  return SETTINGS_NAV.map((group) => {
    const pages: ShownPage[] = [];
    let memoryOpen = false;
    for (const item of group.pages) {
      if (item.children) {
        const children = needle ? item.children.filter((child) => matches(child, needle)) : item.children;
        const open = needle ? children.length > 0 : isMemoryPage(current);
        if (open || (needle ? matches(item, needle) : true)) pages.push({ item, children: open ? children : undefined });
        memoryOpen = open;
      } else if (item.shortcut) {
        if (!memoryOpen && (!needle || matches(item, needle))) pages.push({ item });
      } else if (!needle || matches(item, needle)) {
        pages.push({ item });
      }
    }
    return { heading: group.heading, pages };
  }).filter((group) => group.pages.length > 0);
}

function NavRow({ item, current, child = false }: { item: SettingsNavPage; current: SettingsPageId | null; child?: boolean }) {
  const { Icon } = item;
  // Memory's own row only leads to its Overview; the child row shows which page is open.
  const selected = !item.children && item.page === current;
  return (
    <Link to={settingsPath(item.page)} className={cx("settings-nav-row", child && "settings-nav-row--child")} aria-current={selected ? "page" : undefined}>
      <Icon className="settings-nav-icon" size={child ? 12 : 14} strokeWidth={ICON_STROKE} aria-hidden="true" />
      <span className="settings-nav-label">{t(item.title)}</span>
      {item.isNew ? <Badge tone="new">{t("settings.nav.badge.new")}</Badge> : null}
    </Link>
  );
}

function FooterRow({ link }: { link: SettingsFooterLink }) {
  const client = useClient();
  const toast = useToast();
  const { Icon } = link;
  const content = (
    <>
      <Icon className="settings-nav-icon" size={14} strokeWidth={ICON_STROKE} aria-hidden="true" />
      <span className="settings-nav-label">{t(link.title)}</span>
      {link.external ? <ArrowUpRight className="settings-nav-external" size={18} strokeWidth={1.75} aria-hidden="true" /> : null}
    </>
  );
  if (/^https?:/.test(link.url)) {
    return (
      <a className="settings-nav-row" href={link.url} target="_blank" rel="noopener noreferrer">
        {content}
      </a>
    );
  }
  return (
    <button
      type="button"
      className="settings-nav-row"
      onClick={() =>
        void openInBrowser(client, link.url).then((opened) => {
          if (!opened) toast.notify(t("settings.nav.browserPageUnavailable", { url: link.url }), "error");
        })
      }
    >
      {content}
    </button>
  );
}

/**
 * Aside's Settings column: "Search.." over the grouped pages, Memory's pages under a guide line
 * while one of them is open, and the footer links. ↑/↓ move between rows; ↓ or Enter in the
 * search goes to the first match, Escape clears it.
 */
export function SettingsNav({ current }: { current: SettingsPageId | null }) {
  const navigate = useNavigate();
  const [query, setQuery] = useState("");
  const nav = useRef<HTMLElement>(null);
  const needle = query.trim().toLowerCase();
  const groups = shownGroups(current, needle);
  const footer = needle ? SETTINGS_FOOTER.filter((link) => t(link.title).toLowerCase().includes(needle)) : SETTINGS_FOOTER;

  const rows = () => [...(nav.current?.querySelectorAll<HTMLElement>(".settings-nav-row") ?? [])];
  const onSearchKey = (event: KeyboardEvent<HTMLInputElement>) => {
    if (event.key === "Escape" && query) {
      event.preventDefault();
      setQuery("");
    } else if (event.key === "ArrowDown") {
      event.preventDefault();
      rows()[0]?.focus();
    } else if (event.key === "Enter") {
      const first = groups[0]?.pages[0];
      if (!first) return;
      event.preventDefault();
      navigate(settingsPath(first.children?.[0]?.page ?? first.item.page));
    }
  };
  const onRowKey = (event: KeyboardEvent<HTMLElement>) => {
    if (event.key !== "ArrowDown" && event.key !== "ArrowUp") return;
    const all = rows();
    const at = all.indexOf(event.target as HTMLElement);
    if (at < 0) return;
    event.preventDefault();
    if (event.key === "ArrowUp" && at === 0) nav.current?.querySelector<HTMLInputElement>("input")?.focus();
    else all[Math.min(all.length - 1, event.key === "ArrowDown" ? at + 1 : at - 1)]?.focus();
  };

  return (
    <nav className="settings-nav" aria-label={t("settings.nav.label")} ref={nav} onKeyDown={onRowKey}>
      <label className="settings-nav-search">
        <Search size={12} strokeWidth={ICON_STROKE} aria-hidden="true" />
        <span className="sr-only">{t("settings.nav.searchLabel")}</span>
        <input type="search" value={query} onChange={(event) => setQuery(event.target.value)} onKeyDown={onSearchKey} placeholder={t("settings.nav.searchPlaceholder")} />
      </label>
      {groups.map((group) => (
        <div key={group.heading} className="settings-nav-group" role="group" aria-label={t(group.heading)}>
          <h2 className="settings-nav-heading" aria-hidden="true">
            {t(group.heading)}
          </h2>
          {group.pages.map(({ item, children }) => (
            <div key={`${item.page}${item.shortcut ? "-shortcut" : ""}`} className="settings-nav-item">
              <NavRow item={item} current={current} />
              {children && children.length > 0 ? (
                <div className="settings-nav-children">
                  {children.map((child) => (
                    <NavRow key={child.page + child.title} item={child} current={current} child />
                  ))}
                </div>
              ) : null}
            </div>
          ))}
        </div>
      ))}
      {footer.length > 0 ? (
        <div className="settings-nav-group settings-nav-footer">
          {footer.map((link) => (
            <FooterRow key={link.key} link={link} />
          ))}
        </div>
      ) : null}
      {groups.length === 0 && footer.length === 0 ? <p className="settings-nav-empty">{t("settings.nav.noMatch", { query: query.trim() })}</p> : null}
    </nav>
  );
}

/**
 * `/settings/*`: Settings as a tab in the content card (Aside settings §0–2). The toolbar above,
 * with its `Settings › Page` breadcrumb, is the shell's. Desktop opens General for `/settings`;
 * on a phone `/settings` is the list of pages and each page has a back control to it.
 */
export function SettingsLayout({ onSignOut }: { onSignOut: () => void }) {
  const { pathname } = useLocation();
  const phone = useSyncExternalStore(subscribePhone, phoneNow);
  const page = settingsPageOf(pathname);
  const index = /^\/settings\/*$/.test(pathname);
  const navigationType = useNavigationType();
  const shown = useRef(page);

  // A page picked in the nav (or by back/forward) moves focus to its title so keyboard and screen
  // reader users land on it. Redirects and the first render leave focus alone, and so does someone
  // still typing in the nav search.
  useEffect(() => {
    if (shown.current === page) return;
    shown.current = page;
    if (!page || navigationType === "REPLACE") return;
    if (document.activeElement?.closest(".settings-nav-search")) return;
    document.getElementById("settings-page-title")?.focus();
  }, [page, navigationType]);

  if (!page && !(index && phone)) return <Navigate to={settingsPath("general")} replace />;
  const Body = page ? SETTINGS_BODY[page] : undefined;
  const custom = page ? HEADING[page] : undefined;
  const title = custom ? t(custom.title) : page ? settingsTitle(page) : t("common.settings");
  const bleed = page !== null && FULL_BLEED[page] === true;
  const heading = phone ? null : (
    <>
      <h1 className={bleed ? "sr-only" : "settings-title"} id="settings-page-title" tabIndex={-1} aria-describedby={custom ? "settings-page-help" : undefined}>
        {title}
        {custom ? (
          <span className="settings-title-help" title={t(custom.help)} aria-hidden="true">
            <CircleHelp strokeWidth={ICON_STROKE} />
          </span>
        ) : null}
      </h1>
      {custom ? (
        <p className="sr-only" id="settings-page-help">
          {t(custom.help)}
        </p>
      ) : null}
    </>
  );

  return (
    <div className="settings-layout">
      {phone ? (
        <AppHeader
          heading={
            page ? (
              <div className="settings-phone-heading">
                <Link to="/settings" className="ui-icon-button" aria-label={t("settings.nav.back")} title={t("settings.nav.back")}>
                  <ChevronLeft size={20} strokeWidth={ICON_STROKE} aria-hidden="true" />
                </Link>
                <h1 className="app-title" id="settings-page-title" tabIndex={-1}>
                  {title}
                </h1>
              </div>
            ) : undefined
          }
          title={page ? undefined : title}
        />
      ) : null}
      {phone && page ? null : <SettingsNav current={page} />}
      {page && Body ? (
        <div className="settings-sheet">
          <main className={cx("settings-scroll", bleed && "settings-scroll--bleed")} aria-labelledby="settings-page-title">
            {bleed ? (
              <>
                {heading}
                <Body key={page} onSignOut={onSignOut} />
              </>
            ) : (
              <div className="settings-column" data-page={page}>
                {heading}
                <Body key={page} onSignOut={onSignOut} />
              </div>
            )}
          </main>
        </div>
      ) : null}
    </div>
  );
}
