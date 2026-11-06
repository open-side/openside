import { useCallback, useEffect, useState } from "react";
import type { BrowserPrefs, BrowserPrefValues, UpdateBrowserPrefsRequest } from "../../api/types";
import { useClient } from "../../context";
import { openInBrowser } from "../../hooks/usePersonBrowser";
import { errorMessage } from "../../hooks/useResource";
import { t } from "../../i18n";
import { useToast } from "../../ui";

export interface BrowserSettings {
  /** Current values, or null while loading or when this browser cannot be read. */
  prefs: BrowserPrefValues | null;
  /** Why the rows are disabled, or null when they can change. */
  note: string | null;
  /** The daemon's answer, or null while loading. */
  status: BrowserPrefs["status"] | null;
  update: (patch: UpdateBrowserPrefsRequest) => void;
}

function noteFor(status: BrowserPrefs["status"] | null): string | null {
  if (status === "not_running") return t("personal.browser.notRunning");
  if (status === "unsupported") return t("personal.browser.unsupported");
  return null;
}

/**
 * The person's browser settings (search engine, languages, zoom, cookies…), read from the browser's
 * own profile through the daemon. A change shows at once and rolls back if the browser refuses it.
 */
export function useBrowserSettings(): BrowserSettings {
  const client = useClient();
  const toast = useToast();
  const [state, setState] = useState<BrowserPrefs | null>(null);

  useEffect(() => {
    let cancelled = false;
    client.browserPrefs().then(
      (next) => {
        if (!cancelled) setState(next);
      },
      () => {
        if (!cancelled) setState({ status: "not_running", prefs: null });
      },
    );
    return () => {
      cancelled = true;
    };
  }, [client]);

  const update = useCallback(
    (patch: UpdateBrowserPrefsRequest) => {
      setState((current) => {
        const prefs = current?.prefs;
        if (!prefs) return current;
        const next: BrowserPrefValues = { ...prefs };
        if (patch.accept_languages != null) next.accept_languages = patch.accept_languages;
        if (patch.auto_picture_in_picture != null) next.auto_picture_in_picture = patch.auto_picture_in_picture;
        if (patch.default_zoom_percent != null) next.default_zoom_percent = patch.default_zoom_percent;
        if (patch.search_engine != null) next.search_engine = { ...prefs.search_engine, current: patch.search_engine };
        if (patch.spell_check != null) next.spell_check = patch.spell_check;
        if (patch.third_party_cookies != null) next.third_party_cookies = patch.third_party_cookies;
        if (patch.mini_popup != null) next.mini_popup = patch.mini_popup;
        return { ...current, prefs: next };
      });
      client.updateBrowserPrefs(patch).then(setState, (cause: unknown) => {
        toast.notify(t("personal.browser.saveFailed", { reason: errorMessage(cause) }), "error");
        void client.browserPrefs().then(setState, () => undefined);
      });
    },
    [client, toast],
  );

  return {
    prefs: state?.status === "available" ? (state.prefs ?? null) : null,
    note: state ? noteFor(state.status) : null,
    status: state?.status ?? null,
    update,
  };
}

/** Opens a browser page (chrome://…) in the person's browser, or says how to reach it. */
export function useBrowserPage(): (url: string) => void {
  const client = useClient();
  const toast = useToast();
  return useCallback(
    (url: string) =>
      void openInBrowser(client, url).then((opened) => {
        if (!opened && !/^https?:/i.test(url)) toast.notify(t("personal.browser.pageUnavailable", { url }), "error");
      }),
    [client, toast],
  );
}

/** The 32px / 40px letter tile Account and General show for the person. */
export function Avatar({ name, size }: { name: string; size: 32 | 40 }) {
  const letter = [...name.trim()][0]?.toUpperCase() ?? "?";
  return (
    <span className="personal-avatar" data-size={size} aria-hidden="true">
      {letter}
    </span>
  );
}
