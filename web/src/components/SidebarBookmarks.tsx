import { ChevronDown, ChevronRight, Folder, X } from "lucide-react";
import { useState, type DragEvent } from "react";
import type { PersonBookmark } from "../api/types";
import { useClient } from "../context";
import { bookmarkableUrl, openInBrowser, usePersonBrowser } from "../hooks/usePersonBrowser";
import { t } from "../i18n";
import { cx, ICON_STROKE, useToast } from "../ui";
import { browserFailure } from "./SidebarChats";
import { dragTab, Favicon, TAB_DRAG_TYPE } from "./SidebarTabs";

/**
 * A dropped tab row (title and address) or any dragged link (address only): `null` when nothing
 * link-like was dropped, `"refused"` when the address is not a web page that may be saved.
 */
function dropped(event: DragEvent): { title: string; url: string } | "refused" | null {
  const tab = event.dataTransfer.getData(TAB_DRAG_TYPE);
  let link: { title: string; url: string } | null = null;
  if (tab) {
    try {
      const value: unknown = JSON.parse(tab);
      if (value && typeof value === "object" && "title" in value && "url" in value && typeof value.title === "string" && typeof value.url === "string") link = { title: value.title, url: value.url };
    } catch {
      return "refused";
    }
    if (!link) return "refused";
  } else {
    const url = event.dataTransfer.getData("text/uri-list").split(/\r?\n/).find((line) => line && !line.startsWith("#"));
    if (!url) return null;
    link = { title: url, url };
  }
  const url = bookmarkableUrl(link.url);
  return url ? { title: link.title.trim() || url, url } : "refused";
}

/** Bookmarks: the person's Bookmarks bar, or a drop box while it is empty (`L 10`). Tabs dropped here are saved. */
export function SidebarBookmarks({ onPicked }: { onPicked?: () => void }) {
  const client = useClient();
  const toast = useToast();
  const browser = usePersonBrowser();
  const [over, setOver] = useState(false);
  const accepts = (event: DragEvent) => event.dataTransfer.types.includes(TAB_DRAG_TYPE) || event.dataTransfer.types.includes("text/uri-list");

  async function drop(event: DragEvent) {
    event.preventDefault();
    setOver(false);
    const link = dropped(event);
    if (link === null) return;
    if (link === "refused") {
      toast.notify(t("shell.bookmarks.notSaved"), "error");
      return;
    }
    try {
      await client.createPersonBookmark({ parent_id: browser.bookmarkBar, title: link.title, url: link.url });
      browser.refreshBookmarks();
    } catch (cause) {
      toast.notify(browserFailure(cause), "error");
    }
  }

  async function remove(bookmark: PersonBookmark) {
    try {
      await client.deletePersonBookmark(bookmark.id);
      browser.refreshBookmarks();
    } catch (cause) {
      toast.notify(browserFailure(cause), "error");
    }
  }

  return (
    <section
      className={cx("sidebar-section sidebar-bookmarks", over && "drop-over")}
      aria-labelledby="sidebar-bookmarks-heading"
      onDragOver={(event) => {
        if (!accepts(event)) return;
        event.preventDefault();
        event.dataTransfer.dropEffect = "link";
        setOver(true);
      }}
      onDragLeave={(event) => {
        if (!event.currentTarget.contains(event.relatedTarget as Node | null)) setOver(false);
      }}
      onDrop={(event) => void drop(event)}
    >
      <h2 id="sidebar-bookmarks-heading" className="sidebar-heading">
        {t("shell.bookmarks.heading")}
      </h2>
      {browser.bookmarks.length === 0 ? (
        <p className="bookmark-drop">{t("shell.bookmarks.dropHint")}</p>
      ) : (
        <ul className="sidebar-rows">
          {browser.bookmarks.map((bookmark) => (
            <BookmarkRow key={bookmark.id} bookmark={bookmark} onRemove={(item) => void remove(item)} onPicked={onPicked} />
          ))}
        </ul>
      )}
    </section>
  );
}

function BookmarkRow({ bookmark, child = false, onRemove, onPicked }: { bookmark: PersonBookmark; child?: boolean; onRemove: (bookmark: PersonBookmark) => void; onPicked?: () => void }) {
  const client = useClient();
  const toast = useToast();
  const browser = usePersonBrowser();
  const [open, setOpen] = useState(false);
  const title = bookmark.title.trim() || bookmark.url || t("shell.bookmarks.untitled");
  const { url } = bookmark;

  if (url === null || url === undefined) {
    return (
      <li className="tab-group">
        <button type="button" className="sidebar-row tab-group-toggle" aria-expanded={open} onClick={() => setOpen((value) => !value)}>
          <Folder size={16} strokeWidth={ICON_STROKE} aria-hidden="true" />
          <span className="tab-group-label">{title}</span>
          {open ? <ChevronDown className="tab-group-caret" size={12} strokeWidth={ICON_STROKE} aria-hidden="true" /> : <ChevronRight className="tab-group-caret" size={12} strokeWidth={ICON_STROKE} aria-hidden="true" />}
        </button>
        {open ? (
          <ul className="sidebar-rows tab-group-rows" aria-label={title}>
            {bookmark.children.map((item) => (
              <BookmarkRow key={item.id} bookmark={item} child onRemove={onRemove} onPicked={onPicked} />
            ))}
          </ul>
        ) : null}
      </li>
    );
  }

  const openBookmark = async () => {
    onPicked?.();
    try {
      // A bookmark already open in a tab switches to it, as the browser's own sidebar does.
      const tab = browser.tabs.find((candidate) => candidate.url === url);
      if (tab) await browser.command({ command: "activate", tab_id: tab.tab_id });
      else if (!(await openInBrowser(client, url)) && !bookmarkableUrl(url)) toast.notify(t("shell.browser.notConnected"), "error");
    } catch (cause) {
      toast.notify(browserFailure(cause), "error");
    }
  };

  return (
    <li className={cx("tab-row", child && "tab-row--child")}>
      <button type="button" className="sidebar-row" title={title} draggable onDragStart={(event) => dragTab(event, title, url)} onClick={() => void openBookmark()}>
        <Favicon url={url} />
        <span className="sidebar-row-label">{title}</span>
      </button>
      <button type="button" className="sidebar-row-trailing sidebar-row-action tab-close" aria-label={t("shell.bookmarks.removeNamed", { title })} title={t("shell.bookmarks.remove")} onClick={() => onRemove(bookmark)}>
        <X size={12} strokeWidth={ICON_STROKE} aria-hidden="true" />
      </button>
    </li>
  );
}
