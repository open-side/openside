import { PanelTopOpen } from "lucide-react";
import type { SessionView } from "../state/session";
import { ICON_STROKE, IconButton } from "../ui";
import { useBrowserDock } from "./browserDock";

// Matches the phone breakpoint in ui.css; a phone has no room for a floating preview.
const PHONE_QUERY = "(max-width: 820px)";

/**
 * Header control that brings the conversation's browser back after its PiP was closed: the
 * floating preview on a desktop, the browser sheet on a phone. Shown once the session has a
 * browser state; private sessions never show their browser.
 */
export function BrowserButton({ sessionId, view }: { sessionId: string; view: SessionView }) {
  const { setPip, setSheet } = useBrowserDock(sessionId);
  if (view.incognito || Object.keys(view.browserStates).length === 0) return null;
  return (
    <IconButton
      label="Show browser"
      onClick={() => {
        if (window.matchMedia?.(PHONE_QUERY).matches) setSheet(true);
        else setPip("open");
      }}
    >
      <PanelTopOpen size={20} strokeWidth={ICON_STROKE} aria-hidden="true" />
    </IconButton>
  );
}
