import { useState } from "react";
import { t } from "../i18n";
import { openAddress, useSearchEngine } from "./AppHeader";

/**
 * Home's Search mode (`L 10`): Enter opens a typed address, or searches the browser's default
 * engine for anything else, as the address bar does. Conversations are found from the search
 * palette and Chats instead.
 */
export function HomeSearch() {
  const { known } = useSearchEngine();
  const [query, setQuery] = useState("");
  return (
    <form
      role="search"
      className="home-omnibox"
      onSubmit={(event) => {
        event.preventDefault();
        void known().then((engine) => openAddress(query, engine));
      }}
    >
      <label className="home-omnibox-field">
        <span className="sr-only">{t("shell.homeSearch.label")}</span>
        <input
          type="search"
          value={query}
          onChange={(event) => setQuery(event.target.value)}
          placeholder={t("shell.homeSearch.placeholder")}
          enterKeyHint="go"
          autoComplete="off"
          spellCheck={false}
          // Picking Search is asking to type; the field is the only thing in this mode.
          autoFocus
        />
      </label>
    </form>
  );
}
