import { useEffect, useId, useRef, useState, type KeyboardEvent } from "react";
import { useLocation, useNavigate } from "react-router";
import { loadNewTabMode } from "../appearance";
import { AppHeader } from "../components/AppHeader";
import { Composer } from "../components/Composer";
import { HomeSearch } from "../components/HomeSearch";
import { AccountMenu } from "../components/SidebarAccount";
import { HOME_SUGGESTIONS, SuggestedTasks } from "../components/SuggestedTasks";
import { t, type MessageKey } from "../i18n";
import { setShareDraft, useShareDraft, type ShareDraft } from "../pwa/share";
import { BrandMark, Segmented } from "../ui";
import "./home.css";

type Mode = "ask" | "search";

const MODE_KEYS: Record<string, true> = { ArrowLeft: true, ArrowRight: true, ArrowUp: true, ArrowDown: true, Home: true, End: true, Enter: true, " ": true };

/**
 * `/`, the new tab (`L 10`): the mark, one pill that searches the web (Search) or starts a new chat
 * (Ask), and suggested tasks. Ask creates the chat with the options picked under the pill and opens
 * it once the first message is accepted or parked in the outbox; the pending bubble carries over.
 */
export function HomePage() {
  const navigate = useNavigate();
  const shared = useShareDraft();
  // "New Chat" (sidebar, Mod+Shift+E) navigates to `/` again: those visits start in Ask. The page a
  // new tab loads (react-router's first location, key "default") follows Settings › New tab mode.
  const visit = useLocation().key;
  const [chosen, setChosen] = useState<{ visit: string; mode: Mode }>(() => ({ visit, mode: visit === "default" ? loadNewTabMode() : "ask" }));
  const mode = chosen.visit === visit ? chosen.mode : "ask";
  const setMode = (next: Mode) => setChosen({ visit, mode: next });
  const [picked, setPicked] = useState<ShareDraft | null>(null);
  // Something shared to the app is a task to hand over, whichever mode Home was left in.
  const draft = shared ?? picked;
  const shown: Mode = draft ? "ask" : mode;
  // Both modes focus their field on arrival, which is right after a click; a person arrowing
  // through the toggle keeps focus on it instead.
  const modes = useRef<HTMLDivElement>(null);
  const keyed = useRef(false);
  const refocus = useRef(false);
  const panel = useId();
  const toggle = (next: Mode) => {
    refocus.current = keyed.current;
    keyed.current = false;
    setMode(next);
  };
  useEffect(() => {
    if (!refocus.current) return;
    refocus.current = false;
    modes.current?.querySelector<HTMLElement>('[aria-selected="true"]')?.focus();
  }, [shown]);

  const pick = (id: string, prompt: MessageKey) => {
    setPicked({ id: `${id}-${Date.now()}`, text: t(prompt), files: [] });
    refocus.current = false;
    setMode("ask");
  };
  // Tab flips the mode from an empty field, as the hint beside the toggle says. Once something is
  // typed Tab is the field's again (the @ menu takes it, then the focus order).
  const switchOnTab = (event: KeyboardEvent<HTMLDivElement>) => {
    if (event.key !== "Tab" || event.shiftKey || event.altKey || event.ctrlKey || event.metaKey || event.nativeEvent.isComposing) return;
    const field = event.target;
    if (!(field instanceof HTMLInputElement || field instanceof HTMLTextAreaElement) || field.value !== "") return;
    event.preventDefault();
    refocus.current = false;
    setMode(shown === "search" ? "ask" : "search");
  };

  return (
    <main className="home" data-mode={shown}>
      <AppHeader newTask={false} />
      <div className="home-scroll">
        <AccountMenu placement="home" />
        <div className="home-column">
          <div className="home-mark" role="img" aria-label="openside">
            <BrandMark size={60} />
          </div>
          <h1 className="sr-only">{t("shell.home.title")}</h1>
          <div className="home-omnibox-area" onKeyDownCapture={switchOnTab}>
            <div
              ref={modes}
              className="home-modes"
              // Capture: the toggle's own arrow handler changes the mode during the bubble phase.
              onKeyDownCapture={(event) => {
                keyed.current = MODE_KEYS[event.key] === true;
              }}
              onPointerDown={() => {
                keyed.current = false;
              }}
            >
              <span className="home-switch-hint">
                <kbd>Tab</kbd>
                {t("shell.home.switchHint")}
              </span>
              <Segmented
                label={t("shell.home.modeLabel")}
                kind="tab"
                panel={panel}
                options={[
                  { value: "search", label: t("common.search") },
                  { value: "ask", label: t("shell.home.ask") },
                ]}
                value={shown}
                onChange={toggle}
              />
            </div>
            <div role="tabpanel" id={panel} aria-labelledby={`${panel}-${shown}`}>
              {shown === "search" ? (
                <HomeSearch />
              ) : (
                <Composer
                  variant="home"
                  onSent={(created) => {
                    if (created) navigate(`/sessions/${encodeURIComponent(created)}`);
                  }}
                  onDraftApplied={() => {
                    setMode("ask");
                    if (shared) setShareDraft(null);
                    else setPicked(null);
                  }}
                  draft={draft}
                />
              )}
            </div>
          </div>
          <SuggestedTasks suggestions={HOME_SUGGESTIONS} onPrompt={pick} />
        </div>
      </div>
    </main>
  );
}
