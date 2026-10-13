import { AccessRequestDock } from "./access/AccessRequestDock";
import { useCallback, useEffect, useMemo, useRef, useState, type RefObject } from "react";
import { BrowserRouter, Link, Outlet, Route, Routes, useLocation, type Location } from "react-router";
import { createClient } from "./api/client";
import { relayTransport, type RelayTransport } from "./api/relay/transport";
import { clearLocalSecret, clearToken, loadLocalSecret, loadToken, loadTransport, saveToken } from "./auth";
import { AppHeader } from "./components/AppHeader";
import { CardAttention } from "./components/CardAttention";
import { Sidebar } from "./components/Sidebar";
import { TeamActionsProvider } from "./components/TeamActions";
import { TokenEntry } from "./components/TokenEntry";
import { ClientContext, useClient } from "./context";
import { AgentsContext, CHIEF_ID, useAgents } from "./hooks/useAgents";
import { useSessionList } from "./hooks/useSessionList";
import { useDeliverPending } from "./import/delivery";
import { Landing, OnboardingGate } from "./onboarding/OnboardingGate";
import { OnboardingPage } from "./onboarding/OnboardingPage";
import { AgentPage } from "./pages/AgentPage";
import { HistoryPage } from "./pages/HistoryPage";
import { ManageDialogs } from "./pages/ManageDialogs";
import { backgroundOf, manageTarget } from "./pages/manageRoutes";
import { PairPage } from "./pages/PairPage";
import { SessionPage } from "./pages/SessionPage";
import { useDesktopResync } from "./desktopNotify";
import { usePushPresence } from "./push";
import { PwaRuntime } from "./pwa/PwaRuntime";
import { ShellContext, type Shell } from "./shell";
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

function Layout({ onSignOut, location }: { onSignOut: () => void; location: Location }) {
  const client = useClient();
  usePushPresence(client);
  useDesktopResync(client);
  // Every conversation, for requests that float over another screen; the sidebar shows people.
  const list = useSessionList(client);
  const team = useAgents(client);
  const [drawer, setDrawer] = useState(false);
  const root = useRef<HTMLDivElement>(null);
  // The real location: inside a dialog route, the routes below render the background one.
  const { pathname } = location;
  useVisualViewportHeight(root);

  // Picking a person in the phone drawer can keep the same pathname; any navigation closes it.
  useEffect(() => setDrawer(false), [location.key]);

  const { refresh } = list;
  const { refresh: refreshTeam } = team;
  // Batches left over from an import whose delivery to this browser was interrupted.
  useDeliverPending(client);
  const shell = useMemo<Shell>(
    () => ({
      openDrawer: () => setDrawer(true),
      refreshSessions: () => {
        refresh();
        refreshTeam();
      },
    }),
    [refresh, refreshTeam],
  );
  const openAgent = /^\/agents\/([^/]+)/.exec(pathname)?.[1];
  const visibleSession = manageTarget(pathname)
    ? null
    : openAgent
      ? (team.agents?.find((agent) => agent.id === decodeURIComponent(openAgent))?.home_session_id ?? null)
      : (/^\/sessions\/([^/]+)/.exec(pathname)?.[1] ?? null);

  return (
    <ToastProvider>
      <ShellContext.Provider value={shell}>
        <AgentsContext.Provider value={team}>
          <TeamActionsProvider>
            <div className="shell" ref={root} data-focus={pathname === "/welcome" || undefined}>
              <aside className="shell-sidebar" aria-label="Team sidebar">
                <Sidebar team={team} />
              </aside>
              <div className="shell-content">
                <OnboardingGate>
                  <Outlet />
                </OnboardingGate>
                {list.offline || team.offline ? (
                  <p className="connection-banner" role="status">
                    <Spinner size={14} />
                    Can’t reach openside. Reconnecting…
                  </p>
                ) : null}
                <PwaRuntime />
                <AccessRequestDock />
              </div>
              <Drawer open={drawer} onClose={() => setDrawer(false)} label="Team menu">
                <Sidebar team={team} onClose={() => setDrawer(false)} />
              </Drawer>
              <ManageDialogs location={location} onSignOut={onSignOut} />
              {/* A conversation under a Settings or Workspace dialog is not visible, so its code request floats too. */}
              <CardAttention visibleSession={visibleSession} sessions={list.sessions} />
            </div>
          </TeamActionsProvider>
        </AgentsContext.Provider>
      </ShellContext.Provider>
    </ToastProvider>
  );
}

function NotFound() {
  return (
    <div className="page-frame">
      <AppHeader title="Not found" />
      <main className="not-found">
        <p>Page not found.</p>
        <Link to="/" className="action-link">
          Back to your assistant
        </Link>
      </main>
    </div>
  );
}

/** Stands in underneath a deep-linked dialog route that has no screen to return to. */
const HOME: Location = { pathname: `/agents/${CHIEF_ID}`, search: "", hash: "", state: null, key: "home" };

/** Screens are added here; every screen talks to the daemon only through the API client. */
function AppRoutes({ onSignOut }: { onSignOut: () => void }) {
  const location = useLocation();
  const screen = manageTarget(location.pathname) ? (backgroundOf(location) ?? HOME) : location;
  return (
    <Routes location={screen}>
      <Route element={<Layout onSignOut={onSignOut} location={location} />}>
        <Route index element={<Landing onRoot={location.pathname === "/"} />} />
        <Route path="welcome" element={<OnboardingPage />} />
        <Route path="agents/:id" element={<AgentPage />} />
        <Route path="history" element={<HistoryPage />} />
        <Route path="sessions/:id" element={<SessionPage />} />
        <Route path="*" element={<NotFound />} />
      </Route>
    </Routes>
  );
}

export function App() {
  const [token, setToken] = useState<string | null>(loadToken);
  const [reason, setReason] = useState<string | undefined>();
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
          setReason(localOwner ? "This local browser session ended. Reopen openside-browser." : transport ? "The saved token was rejected. Pair this device again using a new QR code." : "The saved token was rejected. Paste a valid device token.");
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

  if (pairing) return <PairPage onPaired={onPaired} />;

  if (!token || connection.invalid) {
    return (
      <TokenEntry
        pairingRequired={connection.invalid}
        reason={connection.invalid ? "The saved relay connection is incomplete. Pair this device again using a new QR code." : reason}
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
