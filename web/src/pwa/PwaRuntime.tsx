import { Download, Share, X } from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";
import { useLocation, useNavigate } from "react-router";
import { useClient } from "../context";
import { useShell } from "../shell";
import { Button, ICON_STROKE, IconButton, useToast } from "../ui";
import { usePendingTicketBadge } from "./badge";
import { useEdgeSwipe } from "./edgeSwipe";
import { useInstallOffer } from "./install";
import { needsYouTarget, parseLaunch } from "./launch";
import type { FlushResult } from "./outbox";
import { fitFiles, setShareDraft, takeShare } from "./share";
import { useOutboxFlush } from "./useOutbox";
import "./pwa.css";

/** Pause before the install banner so it never competes with the first screen. */
const INSTALL_DELAY_MS = 4000;

/** Opens the app from a shortcut or from the share sheet; the query is removed once handled. */
function useLaunchIntent(): void {
  const { search } = useLocation();
  const navigate = useNavigate();
  const client = useClient();
  const toast = useToast();
  const handled = useRef("");
  // Not per-effect: handling a launch navigates, which re-runs the effect while its requests are in flight.
  const mounted = useRef(true);
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);

  useEffect(() => {
    const intent = parseLaunch(search);
    if (!intent || handled.current === search) return;
    handled.current = search;
    const home = () => navigate({ pathname: "/", search: "" }, { replace: true });
    switch (intent.kind) {
      case "new":
        home();
        break;
      case "share-failed":
        home();
        toast.notify("Couldn’t open what you shared.", "error");
        break;
      case "share":
        home();
        takeShare(intent.id)
          .then((draft) => (draft ? fitFiles(draft.files).then((files) => ({ ...draft, files })) : null))
          .then((draft) => {
            if (!mounted.current) return;
            if (draft) setShareDraft(draft);
            else toast.notify("The shared content is no longer available.", "error");
          })
          .catch(() => mounted.current && toast.notify("Couldn’t open what you shared.", "error"));
        break;
      case "needs-you":
        home();
        client.listTickets("pending").then(
          ({ tickets }) => {
            if (!mounted.current) return;
            const target = needsYouTarget(tickets);
            if (!target) return toast.notify("Nothing is waiting for you.");
            navigate(`/sessions/${encodeURIComponent(target.sessionId)}`, { replace: true });
            if (target.conversations > 1) toast.notify(`${target.conversations} conversations need you.`);
          },
          () => mounted.current && toast.notify("Can’t reach openside.", "error"),
        );
        break;
    }
  }, [search, client, navigate, toast]);
}

function InstallBanner() {
  const { offer, dismiss } = useInstallOffer();
  const [ready, setReady] = useState(false);
  useEffect(() => {
    const timer = window.setTimeout(() => setReady(true), INSTALL_DELAY_MS);
    return () => window.clearTimeout(timer);
  }, []);
  if (!offer || !ready) return null;
  return (
    <aside className="install-banner" aria-label="Install openside">
      {offer.kind === "android" ? (
        <>
          <Download size={18} strokeWidth={ICON_STROKE} aria-hidden="true" />
          <span className="install-text">Install openside for quick access and approval alerts.</span>
          <Button type="button" variant="primary" onClick={() => void offer.install()}>
            Install
          </Button>
        </>
      ) : (
        <>
          <Share size={18} strokeWidth={ICON_STROKE} aria-hidden="true" />
          <span className="install-text">Install openside: tap Share, then Add to Home Screen.</span>
        </>
      )}
      <IconButton label="Dismiss" size="small" onClick={dismiss}>
        <X size={16} strokeWidth={ICON_STROKE} aria-hidden="true" />
      </IconButton>
    </aside>
  );
}

/**
 * Everything that makes the web app behave like an installed one, mounted once inside the
 * signed-in shell: launch shortcuts and shared content, the icon badge, offline-outbox delivery,
 * the swipe-from-edge sidebar and the install banner.
 */
export function PwaRuntime() {
  const client = useClient();
  const shell = useShell();
  const navigate = useNavigate();
  const { pathname } = useLocation();
  const onEmptyScreen = useRef(false);
  onEmptyScreen.current = pathname === "/";
  const toast = useToast();
  useLaunchIntent();
  usePendingTicketBadge(client);
  useEdgeSwipe(shell.openDrawer);

  const { refreshSessions } = shell;
  const onFlushed = useCallback(
    ({ sent, failed, created }: FlushResult) => {
      refreshSessions();
      if (failed) toast.notify("A queued message was refused and was not sent.", "error");
      else if (sent) toast.notify(sent === 1 ? "Queued message sent." : `${sent} queued messages sent.`);
      // A chat started offline opens once it exists, but only if the person is still on the empty screen.
      const only = created[0];
      if (created.length === 1 && only && onEmptyScreen.current) navigate(`/sessions/${encodeURIComponent(only)}`);
    },
    [refreshSessions, toast, navigate],
  );
  useOutboxFlush(client, onFlushed);

  // /welcome is the first-run surface; the banner would sit on its heading.
  return pathname === "/welcome" ? null : <InstallBanner />;
}
