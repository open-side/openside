import { createContext, useContext, useEffect, useRef, useState, type ReactNode } from "react";
import { Navigate, useLocation, useNavigate } from "react-router";
import { mayUse } from "../auth";
import { useClient } from "../context";
import { CHIEF_ID } from "../hooks/useAgents";
import { guideDismissed } from "./dismissed";

// `/` redirects here, so being here right after opening `/` still means the person has not gone anywhere.
const LANDING = `/agents/${CHIEF_ID}`;

// "leaving": the guide was chosen and the move to it is in flight; `/` must not redirect meanwhile.
type Verdict = "deciding" | "leaving" | "settled";
const VerdictContext = createContext<Verdict>("settled");

/**
 * Opens the guide on the first visit to `/` while the daemon still has it pending. It decides
 * once, from where the app was opened: a deep link into a conversation is never interrupted, and
 * leaving `/` before the answer arrives cancels the redirect. `Landing` waits on the verdict so
 * the chief page never shows for a moment before the guide replaces it.
 */
export function OnboardingGate({ children }: { children?: ReactNode }) {
  const client = useClient();
  const navigate = useNavigate();
  const { pathname } = useLocation();
  const here = useRef(pathname);
  here.current = pathname;
  // A token known to lack admin cannot record the outcome, so the guide would never end.
  const asks = useRef(pathname === "/" && mayUse("admin") !== false && !guideDismissed());
  const [verdict, setVerdict] = useState<Verdict>(asks.current ? "deciding" : "settled");

  useEffect(() => {
    if (!asks.current) return;
    let cancelled = false;
    client.onboarding().then(
      (status) => {
        if (cancelled) return;
        if (status.state === "pending" && (here.current === "/" || here.current === LANDING)) {
          navigate("/welcome", { replace: true });
          setVerdict("leaving");
        } else setVerdict("settled");
      },
      () => {
        if (!cancelled) setVerdict("settled");
      },
    );
    return () => {
      cancelled = true;
    };
  }, [client, navigate]);

  useEffect(() => {
    if (verdict === "leaving" && pathname !== "/") setVerdict("settled");
  }, [verdict, pathname]);

  return <VerdictContext.Provider value={verdict}>{children}</VerdictContext.Provider>;
}

/**
 * The `/` route: sends a person to the chief once the gate has had its say. `onRoot` is false
 * while a Settings or Workspace dialog is open over `/`: a redirect then would close it.
 */
export function Landing({ onRoot }: { onRoot: boolean }) {
  const verdict = useContext(VerdictContext);
  return verdict === "settled" && onRoot ? <Navigate to={LANDING} replace /> : null;
}
