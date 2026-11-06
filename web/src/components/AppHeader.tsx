import { ArrowLeft, ArrowRight, KeyRound, Menu, PanelLeftOpen, Puzzle, RotateCw, Search, SquarePen } from "lucide-react";
import { useCallback, useEffect, useRef, useState, type FormEvent, type ReactNode } from "react";
import { useLocation, useNavigate } from "react-router";
import type { BrowserPrefs, Session } from "../api/types";
import { useClient } from "../context";
import { openInBrowser } from "../hooks/usePersonBrowser";
import { t } from "../i18n";
import { settingsBreadcrumb } from "../settings/nav";
import { useShell } from "../shell";
import { BrandMark, cx, ICON_STROKE, IconButton } from "../ui";
import { displaySessionTitle } from "./toolStatus";

const icon = { size: 20, strokeWidth: ICON_STROKE, "aria-hidden": true } as const;
const toolIcon = { size: 16, strokeWidth: ICON_STROKE, "aria-hidden": true } as const;

/**
 * What the toolbar and a sidebar tab row call an openside page: "Settings › Models",
 * "Model selection · Chats". `null` for Home, which shows the address field instead.
 */
export function pageLabel(pathname: string, sessions: readonly Session[] | null): string | null {
  if (pathname === "/" || pathname === "") return null;
  const settings = settingsBreadcrumb(pathname);
  if (settings) return settings.join(" › ");
  const chats = t("shell.sidebar.chats");
  const session = /^\/sessions\/([^/]+)/.exec(pathname)?.[1];
  if (session) {
    const id = decodeURIComponent(session);
    const title = sessions?.find((candidate) => candidate.id === id)?.title;
    return `${title === undefined ? t("shell.toolbar.chat") : displaySessionTitle(title)} · ${chats}`;
  }
  if (pathname.startsWith("/chats")) return chats;
  if (pathname.startsWith("/welcome")) return t("shell.toolbar.welcome");
  if (pathname === "/mini") return t("shell.toolbar.mini");
  return t("shell.notFound.title");
}

export interface SearchEngine {
  name: string;
  /** `{q}` is replaced by the encoded query. */
  url: string;
}

export const DEFAULT_SEARCH_ENGINE: SearchEngine = { name: "Google", url: "https://www.google.com/search?q={q}" };

/**
 * Search pages of the browser's built-in engines, keyed by Chromium's prepopulated engine id
 * (`third_party/search_engines_data/.../prepopulated_engines.json`). The daemon exposes only that id
 * and the display name, which is localised ("네이버", "Яндекс", "百度"), so the id is the key. Regional
 * variants share an id (every Yahoo! is 2, every Yandex 15); each maps to its international page.
 * The name here only stands in when the browser's own (localised) option name is missing.
 */
const ENGINES: Readonly<Record<number, SearchEngine>> = {
  1: DEFAULT_SEARCH_ENGINE,
  2: { name: "Yahoo!", url: "https://search.yahoo.com/search?p={q}" },
  3: { name: "Microsoft Bing", url: "https://www.bing.com/search?q={q}" },
  15: { name: "Yandex", url: "https://yandex.com/search/?text={q}" },
  21: { name: "Baidu", url: "https://www.baidu.com/s?wd={q}" },
  25: { name: "Seznam.cz", url: "https://search.seznam.cz/?q={q}" },
  56: { name: "Sogou", url: "https://www.sogou.com/web?query={q}" },
  67: { name: "Naver", url: "https://search.naver.com/search.naver?query={q}" },
  68: { name: "Daum", url: "https://search.daum.net/search?w=tot&q={q}" },
  88: { name: "360", url: "https://www.so.com/s?q={q}" },
  92: { name: "DuckDuckGo", url: "https://duckduckgo.com/?q={q}" },
  94: { name: "Qwant", url: "https://www.qwant.com/?q={q}" },
  101: { name: "Ecosia", url: "https://www.ecosia.org/search?q={q}" },
  109: { name: "Brave", url: "https://search.brave.com/search?q={q}" },
  113: { name: "Startpage", url: "https://www.startpage.com/sp/search?q={q}" },
};

/** The browser's default engine when it is a known built-in one; `null` sends the caller to its fallback. */
export function searchEngineOf(prefs: BrowserPrefs): SearchEngine | null {
  const engines = prefs.status === "available" ? prefs.prefs?.search_engine : undefined;
  const id = engines?.current;
  const known = id == null ? undefined : ENGINES[id];
  if (id == null || !known) return null;
  const name = engines?.options.find((option) => option.id === id)?.name.trim();
  return { name: name || known.name, url: known.url };
}

// A host the way an address bar recognises one without a scheme: localhost, an IPv6 literal, a dotted
// name (Unicode labels allowed), or a single intranet label followed by a port or a slash
// ("intranet:8080", "go/link"); then an optional port and path.
const BARE_ADDRESS = /^(?:localhost|\[[0-9a-f:.]+\]|[\p{L}\p{N}_-]+(?:\.[\p{L}\p{N}_-]+)+|[\p{L}\p{N}_-]+(?=:\d|\/))(?::\d{1,5})?(?:[/?#]\S*)?$/iu;
// Local, literal and single-label (intranet) hosts rarely serve https, so they open over http as a browser does.
const PLAIN_HTTP_HOST = /^(?:localhost|\[[0-9a-f:.]+\]|\d{1,3}(?:\.\d{1,3}){3}|[\p{L}\p{N}_-]+)(?:[:/?#]|$)/iu;

function bareAddress(value: string): string | null {
  if (!BARE_ADDRESS.test(value)) return null;
  const host = value.split(/[:/?#]/, 1)[0] ?? "";
  // "1.5" or "3.14/2" is a query, not an address: an all-numeric host must be a full IPv4.
  if (/^[\d.]+$/.test(host) && !/^\d{1,3}(?:\.\d{1,3}){3}$/.test(host)) return null;
  return `${PLAIN_HTTP_HOST.test(value) ? "http" : "https"}://${value}`;
}

/**
 * A typed address opens as typed; anything else is searched for with `engine`, as an address bar does.
 * Text with a space is always a search, even after "https://". Only http(s) ever navigates:
 * `javascript:`, `data:`, `file:` and other schemes are searched as text.
 */
export function addressTarget(text: string, engine: SearchEngine = DEFAULT_SEARCH_ENGINE): string | null {
  const value = text.trim();
  if (!value) return null;
  const typed = /\s/.test(value) ? null : /^https?:\/\//i.test(value) ? value : bareAddress(value);
  if (typed) {
    try {
      // The URL parser punycodes Unicode hosts and normalises the rest; an invalid host throws.
      const url = new URL(typed);
      if (url.protocol === "http:" || url.protocol === "https:") return url.href;
    } catch {
      // Not a valid address after all: search for the text instead.
    }
  }
  return engine.url.replace("{q}", encodeURIComponent(value));
}

/** Leave the app for what was typed into an address field (the toolbar's or Home's Search). */
export function openAddress(text: string, engine: SearchEngine): void {
  const target = addressTarget(text, engine);
  if (target) window.location.assign(target);
}

// A daemon that never answers must not leave Enter doing nothing: search with the fallback instead.
const ENGINE_WAIT_MS = 1500;

export interface SearchEngineState {
  /** Google until the browser's own default has been read (the toolbar's placeholder names it). */
  engine: SearchEngine;
  /** The browser's default once read, so a submit typed before the answer still uses it; Google when unknown, unreadable or slow. */
  known: () => Promise<SearchEngine>;
}

/** The browser's default search engine, read once. Shared by the toolbar and Home's Search mode. */
export function useSearchEngine(): SearchEngineState {
  const client = useClient();
  const [engine, setEngine] = useState(DEFAULT_SEARCH_ENGINE);
  const pending = useRef<Promise<SearchEngine> | null>(null);
  useEffect(() => {
    let live = true;
    const read = client.browserPrefs().then(
      (prefs) => searchEngineOf(prefs) ?? DEFAULT_SEARCH_ENGINE,
      () => DEFAULT_SEARCH_ENGINE,
    );
    pending.current = read;
    void read.then((found) => {
      if (live) setEngine(found);
    });
    return () => {
      live = false;
    };
  }, [client]);
  const known = useCallback(() => {
    const read = pending.current;
    if (!read) return Promise.resolve(DEFAULT_SEARCH_ENGINE);
    const slow = Promise.withResolvers<SearchEngine>();
    setTimeout(() => slow.resolve(DEFAULT_SEARCH_ENGINE), ENGINE_WAIT_MS);
    return Promise.race([read, slow.promise]);
  }, []);
  return { engine, known };
}

interface NavigationState {
  canGoBack: boolean;
  canGoForward: boolean;
}

/** The Navigation API's view of this tab's history; browsers without it leave both enabled. */
function readNavigation(): NavigationState {
  const navigation: unknown = "navigation" in window ? window.navigation : undefined;
  if (navigation && typeof navigation === "object" && "canGoBack" in navigation && "canGoForward" in navigation) {
    return { canGoBack: navigation.canGoBack === true, canGoForward: navigation.canGoForward === true };
  }
  return { canGoBack: true, canGoForward: true };
}

function useHistoryState(): NavigationState {
  const { key } = useLocation();
  const [state, setState] = useState(readNavigation);
  // Re-read after every route change: the router has pushed or popped an entry by then.
  useEffect(() => setState(readNavigation()), [key]);
  return state;
}

/**
 * The content card's top row on desktop (`L 10`, `L 20`): history buttons, the page's breadcrumb
 * (or the address field on Home), and the Passwords and Extensions buttons.
 */
export function ShellToolbar({ label }: { label: string | null }) {
  const client = useClient();
  const shell = useShell();
  const navigate = useNavigate();
  const { pathname } = useLocation();
  const { canGoBack, canGoForward } = useHistoryState();
  const [address, setAddress] = useState("");
  const { engine, known } = useSearchEngine();
  const home = label === null;

  const submit = (event: FormEvent) => {
    event.preventDefault();
    void known().then((found) => openAddress(address, found));
  };

  return (
    <div className="shell-toolbar" role="toolbar" aria-label={t("shell.toolbar.label")}>
      {shell.sidebarCollapsed ? (
        <IconButton label={t("shell.header.openSidebar")} className="toolbar-button" size="small" onClick={shell.toggleSidebar}>
          <PanelLeftOpen {...toolIcon} />
        </IconButton>
      ) : null}
      <IconButton label={t("shell.toolbar.back")} className="toolbar-button" size="small" disabled={!canGoBack} onClick={() => history.back()}>
        <ArrowLeft {...toolIcon} />
      </IconButton>
      <IconButton label={t("shell.toolbar.forward")} className="toolbar-button" size="small" disabled={!canGoForward} onClick={() => history.forward()}>
        <ArrowRight {...toolIcon} />
      </IconButton>
      <IconButton label={t("shell.toolbar.reload")} className="toolbar-button" size="small" disabled={pathname === "/"} onClick={() => window.location.reload()}>
        <RotateCw {...toolIcon} />
      </IconButton>
      {home ? (
        <form className="toolbar-address" role="search" onSubmit={submit}>
          <Search className="toolbar-address-icon" size={14} strokeWidth={ICON_STROKE} aria-hidden="true" />
          <input value={address} onChange={(event) => setAddress(event.target.value)} placeholder={t("shell.toolbar.addressPlaceholder", { engine: engine.name })} aria-label={t("shell.toolbar.address")} spellCheck={false} autoComplete="off" />
        </form>
      ) : (
        <nav className="toolbar-breadcrumb" aria-label={t("shell.toolbar.breadcrumb")}>
          <BrandMark size={15} />
          <span className="toolbar-app">openside</span>
          <span className="toolbar-divider" aria-hidden="true" />
          <span className="toolbar-path" title={label}>
            {label}
          </span>
        </nav>
      )}
      <div className="toolbar-end">
        <IconButton label={t("shell.toolbar.passwords")} className="toolbar-button" size="small" onClick={() => navigate("/settings/passwords")}>
          <KeyRound {...toolIcon} />
        </IconButton>
        <IconButton label={t("shell.toolbar.extensions")} className="toolbar-button" size="small" onClick={() => void openInBrowser(client, "chrome://extensions")}>
          <Puzzle {...toolIcon} />
        </IconButton>
      </div>
    </div>
  );
}

export interface AppHeaderProps {
  title?: string;
  subtitle?: ReactNode;
  /** Icon buttons on the right (the thread menu). */
  actions?: ReactNode;
  titleClassName?: string;
  /** Replaces the plain title, e.g. a person's avatar and name that opens their profile. */
  heading?: ReactNode;
  /** A New chat button before the title. Off on Home, which is the new chat. */
  newTask?: boolean;
}

/** A screen's own header under the toolbar: 44px on desktop, 64px on a phone with the drawer button. */
export function AppHeader({ title, subtitle, actions, titleClassName, heading, newTask = true }: AppHeaderProps) {
  const shell = useShell();
  const navigate = useNavigate();
  return (
    <header className="app-header">
      <div className="app-header-start">
        <IconButton label={t("shell.header.openMenu")} className="drawer-only" onClick={shell.openDrawer}>
          <Menu {...icon} />
        </IconButton>
        {newTask ? (
          <IconButton label={t("shell.sidebar.newChat")} className="app-header-new" size="small" onClick={() => navigate("/")}>
            <SquarePen {...toolIcon} />
          </IconButton>
        ) : null}
        {heading ??
          (title ? (
            <div className="app-header-heading">
              <h1 className={cx("app-title", titleClassName)} title={title}>
                {title}
              </h1>
              {subtitle && <p className="app-subtitle">{subtitle}</p>}
            </div>
          ) : null)}
      </div>
      <div className="app-header-actions">{actions}</div>
    </header>
  );
}
