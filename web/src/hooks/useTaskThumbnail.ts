import { useEffect, useState } from "react";
import { useClient } from "../context";
import type { BrowserStateView, SessionView } from "../state/session";

/** The Task's last picture of its tab, fetched once per thumbnail id; it may be withheld. */
export function useTaskThumbnail(sessionId: string, thumbnailId: string | undefined, enabled: boolean): string | null {
  const client = useClient();
  const [thumbnail, setThumbnail] = useState<{ id: string; url: string } | null>(null);
  useEffect(() => {
    if (!thumbnailId || !enabled) return;
    const controller = new AbortController();
    let url: string | undefined;
    void client
      .browserThumbnail(sessionId, thumbnailId, controller.signal)
      .then((blob) => {
        if (controller.signal.aborted) return;
        url = URL.createObjectURL(blob);
        setThumbnail({ id: thumbnailId, url });
      })
      .catch(() => {
        /* A thumbnail may expire or be withheld after a secret fill. */
      });
    return () => {
      controller.abort();
      if (url) URL.revokeObjectURL(url);
    };
  }, [client, sessionId, thumbnailId, enabled]);
  return thumbnail && thumbnail.id === thumbnailId ? thumbnail.url : null;
}

/** The newest `browser.state` of a Task's tab: the highest `seq` among the states it owns. */
export function latestTaskState(view: SessionView, taskId: string): BrowserStateView | undefined {
  let latest: BrowserStateView | undefined;
  for (const state of Object.values(view.browserStates)) {
    if (state.task_id === taskId && (!latest || state.seq > latest.seq)) latest = state;
  }
  return latest;
}
