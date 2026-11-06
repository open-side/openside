import { createContext, useCallback, useContext, useMemo, useState } from "react";

/** What the thread's right-hand pane shows: its empty state, a file of this chat, or a side chat. */
export type SplitContent = { kind: "empty" } | { kind: "file"; path: string; title?: string } | { kind: "chat"; sessionId: string };

export interface SplitPaneApi {
  open: boolean;
  content: SplitContent;
  /** Opens the pane on `content`. */
  show: (content: SplitContent) => void;
  close: () => void;
  toggle: () => void;
}

/** Present only beside a desktop thread that can split; without it, files open as sheets. */
export const SplitPaneContext = createContext<SplitPaneApi | null>(null);

export const useSplitPane = () => useContext(SplitPaneContext);

const EMPTY: SplitContent = { kind: "empty" };

/** One thread's pane state; a different chat starts closed. */
export function useSplitPaneState(): SplitPaneApi {
  const [open, setOpen] = useState(false);
  const [content, setContent] = useState<SplitContent>(EMPTY);
  const show = useCallback((next: SplitContent) => {
    setContent(next);
    setOpen(true);
  }, []);
  const close = useCallback(() => setOpen(false), []);
  const toggle = useCallback(() => setOpen((current) => !current), []);
  return useMemo(() => ({ open, content, show, close, toggle }), [open, content, show, close, toggle]);
}
