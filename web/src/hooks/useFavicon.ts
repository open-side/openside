import { useEffect, useState } from "react";
import type { Client } from "../api/client";
import { useClient } from "../context";

// One object URL per origin for the page's lifetime: sidebar rows re-render every poll and must
// not refetch or leak a blob each time. A miss (404, no cache entry) is remembered as `null` too.
const cache = new Map<string, Promise<string | null>>();

function load(client: Client, origin: string): Promise<string | null> {
  let entry = cache.get(origin);
  if (!entry) {
    entry = client.faviconBlob(origin).then(
      (blob) => URL.createObjectURL(blob),
      () => null,
    );
    cache.set(origin, entry);
  }
  return entry;
}

/** The daemon's cached favicon for an exact site origin, as an object URL; `null` while loading or when it has none. */
export function useFavicon(origin: string | null): string | null {
  const client = useClient();
  const [icon, setIcon] = useState<{ origin: string; url: string | null } | null>(null);
  useEffect(() => {
    if (!origin) return;
    let live = true;
    void load(client, origin).then((url) => {
      if (live) setIcon({ origin, url });
    });
    return () => {
      live = false;
    };
  }, [client, origin]);
  return icon && icon.origin === origin ? icon.url : null;
}

/** `origin` of an http(s) URL, or `null` for anything else. */
export function originOf(url: string): string | null {
  try {
    const parsed = new URL(url);
    return parsed.protocol === "http:" || parsed.protocol === "https:" ? parsed.origin : null;
  } catch {
    return null;
  }
}
