import { useEffect, useRef } from "react";
import { useLocation, useNavigate } from "react-router";
import { mayUse } from "../auth";
import { useClient } from "../context";
import { guideDismissed } from "./dismissed";

/**
 * Opens the guide on the first visit to `/` while the daemon still has it pending. It decides
 * once, from where the app was opened: a deep link into a conversation is never interrupted, and
 * leaving `/` before the answer arrives cancels the redirect.
 */
export function OnboardingGate() {
  const client = useClient();
  const navigate = useNavigate();
  const { pathname } = useLocation();
  const here = useRef(pathname);
  here.current = pathname;
  const openedAt = useRef(pathname);

  useEffect(() => {
    // A token known to lack admin cannot record the outcome, so the guide would never end.
    if (openedAt.current !== "/" || mayUse("admin") === false || guideDismissed()) return;
    let cancelled = false;
    client.onboarding().then(
      (status) => {
        if (!cancelled && status.state === "pending" && here.current === "/") navigate("/welcome", { replace: true });
      },
      () => undefined,
    );
    return () => {
      cancelled = true;
    };
  }, [client, navigate]);

  return null;
}
