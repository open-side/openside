import { useCallback, useEffect, useMemo, useRef, useState, type RefObject } from "react";
import { BrowserRouter, Link, Outlet, Route, Routes, useLocation, type Location } from "react-router";
import { createClient } from "./api/client";
import { clearToken, loadToken, saveToken } from "./auth";
import { AppHeader } from "./components/AppHeader";
import { OtpDock } from "./components/OtpDock";
import { Sidebar } from "./components/Sidebar";
import { TokenEntry } from "./components/TokenEntry";
import { ClientContext, useClient } from "./context";
import { useSessionList } from "./hooks/useSessionList";
import { ManageDialogs } from "./pages/ManageDialogs";
import { backgroundOf, manageTarget } from "./pages/manageRoutes";
import { NewChatPage } from "./pages/NewChatPage";
import { PairPage } from "./pages/PairPage";
import { SessionPage } from "./pages/SessionPage";
import { usePushPresence } from "./push";
import { ShellContext, type Shell } from "./shell";
import { Drawer, ToastProvider } from "./ui";

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
  const list = useSessionList(client);
  const [drawer, setDrawer] = useState(false);
  const root = useRef<HTMLDivElement>(null);
  // The real location: inside a dialog route, the routes below render the background one.
  const { pathname } = location;
  useVisualViewportHeight(root);

  useEffect(() => setDrawer(false), [pathname]);

  const { refresh } = list;
  const shell = useMemo<Shell>(() => ({ openDrawer: () => setDrawer(true), refreshSessions: refresh }), [refresh]);

  return (
    <ToastProvider>
      <ShellContext.Provider value={shell}>
        <div className="shell" ref={root}>
          <aside className="shell-sidebar" aria-label="Conversation sidebar">
            <Sidebar list={list} />
          </aside>
          <div className="shell-content">
            <Outlet />
          </div>
          <Drawer open={drawer} onClose={() => setDrawer(false)} label="Conversations menu">
            <Sidebar list={list} onClose={() => setDrawer(false)} />
          </Drawer>
          <ManageDialogs location={location} onSignOut={onSignOut} />
          {/* A conversation under a Settings or Workspace dialog is not visible, so its code request floats too. */}
          <OtpDock visibleSession={manageTarget(pathname) ? null : (/^\/sessions\/([^/]+)/.exec(pathname)?.[1] ?? null)} />
        </div>
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
        <Link to="/">Start a new chat</Link>
      </main>
    </div>
  );
}

/** Stands in underneath a deep-linked dialog route that has no screen to return to. */
const HOME: Location = { pathname: "/", search: "", hash: "", state: null, key: "home" };

/** Screens are added here; every screen talks to the daemon only through the API client. */
function AppRoutes({ onSignOut }: { onSignOut: () => void }) {
  const location = useLocation();
  const screen = manageTarget(location.pathname) ? (backgroundOf(location) ?? HOME) : location;
  return (
    <Routes location={screen}>
      <Route element={<Layout onSignOut={onSignOut} location={location} />}>
        <Route index element={<NewChatPage />} />
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
  const client = useMemo(
    () =>
      createClient({
        token: () => token,
        onUnauthorized: () => {
          clearToken();
          setReason("The saved token was rejected. Paste a valid device token.");
          setToken(null);
        },
      }),
    [token],
  );
  const onPaired = useCallback((value: string) => {
    setReason(undefined);
    setToken(value);
    setPairing(false);
  }, []);

  if (pairing) return <PairPage onPaired={onPaired} />;

  if (!token) {
    return (
      <TokenEntry
        reason={reason}
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
