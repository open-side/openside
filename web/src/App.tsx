import { AccessRequestDock } from "./access/AccessRequestDock";
import { useCallback, useEffect, useMemo, useRef, useState, type RefObject } from "react";
import { BrowserRouter, Link, Navigate, Outlet, Route, Routes, useLocation, useNavigate } from "react-router";
import { createClient } from "./api/client";
import { relayTransport, type RelayTransport } from "./api/relay/transport";
import { clearLocalSecret, clearToken, loadLocalSecret, loadToken, loadTransport, saveToken } from "./auth";
import { AppHeader, pageLabel, ShellToolbar } from "./components/AppHeader";
import { CardAttention } from "./components/CardAttention";
import { SearchPalette } from "./components/SearchPalette";
import { Sidebar } from "./components/Sidebar";
import { TeamActionsProvider } from "./components/TeamActions";
import { TokenEntry } from "./components/TokenEntry";
import { ClientContext, useClient } from "./context";
import { AgentTabsContext, AgentTabsSettledContext, useAgentTabs } from "./hooks/useAgentTabs";
import { AgentsContext, useAgents } from "./hooks/useAgents";
import { PersonBrowserContext, usePersonBrowserState } from "./hooks/usePersonBrowser";
import { useSessionList } from "./hooks/useSessionList";
import { useDeliverPending } from "./import/delivery";
import { Landing, OnboardingGate } from "./onboarding/OnboardingGate";
import { OnboardingPage } from "./onboarding/OnboardingPage";
import { ChatsPage } from "./pages/ChatsPage";
import { HomePage } from "./pages/HomePage";
import { MiniPage } from "./pages/MiniPage";
import { LEGACY_SETTINGS_PATHS, settingsPath } from "./settings/nav";
import { SettingsLayout } from "./settings/SettingsLayout";
import { PairPage } from "./pages/PairPage";
import { SharePage } from "./pages/SharePage";
import { SessionPage } from "./pages/SessionPage";
import { useDesktopResync } from "./desktopNotify";
import { useCompletionSound } from "./completionSound";
import { usePushPresence } from "./push";
import { PwaRuntime } from "./pwa/PwaRuntime";
import { ShellContext, type Shell } from "./shell";
import { useShortcuts } from "./shortcuts";
import { t, type MessageKey } from "./i18n";
import { Drawer, Spinner, ToastProvider } from "./ui";

/**
 * iOS Safari keeps the layout viewport when the keyboard opens; sizing the shell to the visual
 * viewport keeps the composer above the keyboard instead of behind it.
 */
function useVisualViewportHeight(root: RefObject<HTMLDivElement | null>) {
  useEffect(() => {
    const viewport = window.visualViewport;
    const element = root.current;
    if (!viewport || !element) return;
    const sync = () => element.style.setProperty("--app-height", `${Math.round(viewport.height)}px`);
    sync();
    viewport.addEventListener("resize", sync);
    return () => viewport.removeEventListener("resize", sync);
  }, [root]);
}

const SIDEBAR_KEY = "openside.sidebar";

function Layout() {
  const location = useLocation();
  const navigate = useNavigate();
  const client = useClient();
  usePushPresence(client);
  useDesktopResync(client);
  // Every conversation: the sidebar's Chats, and requests that float over another screen.
  const list = useSessionList(client);
  useCompletionSound(client, list.sessions);
  const team = useAgents(client);
  // One poller each for both sidebars: the desktop column stays mounted while the phone drawer is open.
  const { tabs, settled: tabsSettled } = useAgentTabs(client);
  const personBrowser = usePersonBrowserState(client);
  const [drawer, setDrawer] = useState(false);
  const [search, setSearch] = useState(false);
  const [sidebarCollapsed, setSidebarCollapsed] = useState(() => localStorage.getItem(SIDEBAR_KEY) === "collapsed");
  const root = useRef<HTMLDivElement>(null);
  const { pathname, search: query, hash } = location;
  const mini = pathname === "/mini";
  useVisualViewportHeight(root);

  // The extension's palette command lands here as `#search`; open the palette and drop the hash so
  // a reload or Back does not open it again.
  useEffect(() => {
    if (hash !== "#search") return;
    setSearch(true);
    navigate({ pathname, search: query }, { replace: true });
  }, [hash, pathname, query, navigate]);

  // Picking a person in the phone drawer can keep the same pathname; any navigation closes it.
  useEffect(() => setDrawer(false), [location.key]);

  const { refresh } = list;
  const { refresh: refreshTeam } = team;
  // Batches left over from an import whose delivery to this browser was interrupted.
  useDeliverPending(client);
  const toggleSidebar = useCallback(() => setSidebarCollapsed((collapsed) => !collapsed), []);
  useEffect(() => {
    if (sidebarCollapsed) localStorage.setItem(SIDEBAR_KEY, "collapsed");
    else localStorage.removeItem(SIDEBAR_KEY);
  }, [sidebarCollapsed]);
  const openDrawer = useCallback(() => setDrawer(true), []);
  const openSearch = useCallback(() => setSearch(true), []);
  useShortcuts({ toggleSidebar, openDrawer, openSearch });
  const needsYou = (list.sessions ?? []).filter((session) => session.status === "waiting_human" && !session.archived).length;
  const shell = useMemo<Shell>(
    () => ({
      openDrawer,
      openSearch,
      refreshSessions: () => {
        refresh();
        refreshTeam();
      },
      needsYou,
      sidebarCollapsed,
      toggleSidebar,
    }),
    [openDrawer, openSearch, refresh, refreshTeam, needsYou, sidebarCollapsed, toggleSidebar],
  );
  const visibleSession = /^\/sessions\/([^/]+)/.exec(pathname)?.[1] ?? null;
  const sessions = list.sessions;
  const labelOf = useCallback((path: string) => pageLabel(path, sessions), [sessions]);

  return (
    <ToastProvider>
      <ShellContext.Provider value={shell}>
        <AgentsContext.Provider value={team}>
          <AgentTabsContext.Provider value={tabs}>
          <AgentTabsSettledContext.Provider value={tabsSettled}>
          <PersonBrowserContext.Provider value={personBrowser}>
          <TeamActionsProvider>
            <div className="shell" ref={root} data-sidebar={sidebarCollapsed ? "collapsed" : "open"} data-focus={pathname === "/welcome" || undefined} data-mini={mini || undefined}>
              <aside className="shell-sidebar" aria-label={t("shell.app.sidebarLabel")}>
                <Sidebar team={team} tabs={tabs} sessions={list.sessions} pageLabel={labelOf} />
              </aside>
              <div className="shell-content">
                {pathname === "/welcome" || mini ? null : <ShellToolbar label={labelOf(pathname)} />}
                <OnboardingGate>
                  <Outlet />
                </OnboardingGate>
                {list.offline || team.offline ? (
                  <p className="connection-banner" role="status">
                    <Spinner size={14} />
                    {t("shell.app.offlineBanner")}
                  </p>
                ) : null}
                <PwaRuntime />
                <AccessRequestDock />
              </div>
              <Drawer open={drawer} onClose={() => setDrawer(false)} label={t("shell.app.drawerLabel")}>
                <Sidebar team={team} tabs={tabs} sessions={list.sessions} pageLabel={labelOf} onClose={() => setDrawer(false)} />
              </Drawer>
              <SearchPalette open={search} onClose={() => setSearch(false)} tabs={tabs} pageLabel={labelOf} />
              {/* Every other conversation's request floats over the screen. */}
              <CardAttention visibleSession={visibleSession} sessions={list.sessions} />
            </div>
          </TeamActionsProvider>
          </PersonBrowserContext.Provider>
          </AgentTabsSettledContext.Provider>
          </AgentTabsContext.Provider>
        </AgentsContext.Provider>
      </ShellContext.Provider>
    </ToastProvider>
  );
}

function NotFound() {
  return (
    <div className="page-frame">
      <AppHeader title={t("shell.notFound.title")} />
      <main className="not-found">
        <p>{t("shell.notFound.body")}</p>
        <Link to="/" className="action-link">
          {t("shell.notFound.backHome")}
        </Link>
      </main>
    </div>
  );
}

/** Screens are added here; every screen talks to the daemon only through the API client. */
function AppRoutes({ onSignOut }: { onSignOut: () => void }) {
  return (
    <Routes>
      <Route element={<Layout />}>
        <Route
          index
          element={
            <Landing>
              <HomePage />
            </Landing>
          }
        />
        <Route path="welcome" element={<OnboardingPage />} />
        <Route path="mini" element={<MiniPage />} />
        <Route path="chats" element={<ChatsPage />} />
        <Route path="sessions/:id" element={<SessionPage />} />
        <Route path="settings/*" element={<SettingsLayout onSignOut={onSignOut} />} />
        {/* Deep links from before Settings became a tab, from bookmarks, notifications and the CLI. */}
        {LEGACY_SETTINGS_PATHS.map(([from, page]) => (
          <Route key={from} path={from} element={<Navigate to={settingsPath(page)} replace />} />
        ))}
        <Route path="*" element={<NotFound />} />
      </Route>
    </Routes>
  );
}

export function App() {
  const [token, setToken] = useState<string | null>(loadToken);
  const [reason, setReason] = useState<MessageKey | undefined>();
  // `/pair#<code>` works with or without a saved token: pairing replaces it.
  const [pairing, setPairing] = useState(() => window.location.pathname === "/pair");
  // A relay-paired device talks to the daemon through the relay that served this page;
  // re-read on every token change because pairing stores the transport with the token.
  const connection = useMemo((): { transport: RelayTransport | null; invalid: boolean } => {
    if (!token) return { transport: null, invalid: false };
    try {
      const target = loadTransport();
      if (!target && document.querySelector('meta[name="openside-transport"]')?.getAttribute("content") === "relay") {
        return { transport: null, invalid: true };
      }
      return { transport: target ? relayTransport(target) : null, invalid: false };
    } catch {
      return { transport: null, invalid: true };
    }
  }, [token]);
  const transport = connection.transport;
  useEffect(() => () => transport?.close(), [transport]);
  const client = useMemo(
    () =>
      createClient({
        token: () => token,
        fetch: transport?.fetch,
        relay: transport !== null,
        localSecret: loadLocalSecret,
        webSocket: transport?.webSocket,
        onUnauthorized: () => {
          const localOwner = loadLocalSecret() !== null;
          // Another launcher tab may already have stored the newly rotated owner token.
          if (loadToken() === token) clearToken();
          else clearLocalSecret();
          setReason(localOwner ? "shell.auth.localSessionEnded" : transport ? "shell.auth.relayTokenRejected" : "shell.auth.tokenRejected");
          setToken(null);
        },
      }),
    [token, transport],
  );
  const onPaired = useCallback((value: string) => {
    setReason(undefined);
    setToken(value);
    setPairing(false);
  }, []);

  // A shared chat is public: it never asks for or uses this browser's device token.
  if (window.location.pathname === "/share") return <SharePage />;
  if (pairing) return <PairPage onPaired={onPaired} />;

  if (!token || connection.invalid) {
    return (
      <TokenEntry
        pairingRequired={connection.invalid}
        reason={connection.invalid ? t("shell.auth.relayIncomplete") : reason && t(reason)}
        onToken={(value) => {
          saveToken(value);
          setReason(undefined);
          setToken(value);
        }}
      />
    );
  }

  return (
    <ClientContext.Provider value={client}>
      <BrowserRouter>
        <AppRoutes
          onSignOut={() => {
            clearToken();
            setToken(null);
          }}
        />
      </BrowserRouter>
    </ClientContext.Provider>
  );
}
