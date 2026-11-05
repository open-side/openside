import { useEffect, useRef, useState } from "react";
import { useLocation, useNavigate } from "react-router";
import { AppHeader } from "../components/AppHeader";
import { Composer } from "../components/Composer";
import { HomeSearch } from "../components/HomeSearch";
import { RecentTasks } from "../components/RecentTasks";
import { HOME_SUGGESTIONS, SuggestedTasks, type Suggestion } from "../components/SuggestedTasks";
import { people, useTeam } from "../hooks/useAgents";
import { setShareDraft, useShareDraft, type ShareDraft } from "../pwa/share";
import { BrandMark, Segmented, Spinner } from "../ui";

type Mode = "ask" | "search";

const MODES = [
  { value: "search", label: "Search" },
  { value: "ask", label: "Ask" },
] as const;
const MODE_KEYS: Record<string, true> = { ArrowLeft: true, ArrowRight: true, ArrowUp: true, ArrowDown: true, Home: true, End: true, Enter: true, " ": true };

/**
 * `/`: hand the assistant a task, or find an earlier conversation. Ask sends into the
 * assistant's one continuous thread (or, with a per-request override, a fresh session), which
 * opens once it is accepted or parked in the outbox; the pending bubble carries over.
 */
export function HomePage() {
  const navigate = useNavigate();
  const team = useTeam();
  const chief = people(team.agents ?? []).chief;
  const shared = useShareDraft();
  // "New task" (sidebar, Mod+Shift+E) navigates to `/` again: every visit starts in Ask.
  const visit = useLocation().key;
  const [chosen, setChosen] = useState<{ visit: string; mode: Mode }>({ visit, mode: "ask" });
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

  if (!chief) {
    return (
      <main className="home">
        <AppHeader newTask={false} />
        <div className="home-loading">{team.offline ? null : <Spinner label="Loading" />}</div>
      </main>
    );
  }
  const thread = `/agents/${encodeURIComponent(chief.id)}`;
  const pick = (suggestion: Suggestion) => {
    setPicked({ id: `${suggestion.id}-${Date.now()}`, text: suggestion.prompt, files: [] });
    refocus.current = false;
    setMode("ask");
  };
  return (
    <main className="home" data-mode={shown}>
      <AppHeader newTask={false} />
      <div className="home-scroll thin-scroll">
        <div className="home-column">
          <div className="home-mark" role="img" aria-label="openside">
            <BrandMark size={56} />
          </div>
          <h1 className="home-title sr-only">What should we do?</h1>
          <div className="home-omnibox-area">
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
              <Segmented label="Home mode" kind="tab" options={[...MODES]} value={shown} onChange={toggle} />
            </div>
            {shown === "search" ? (
              <HomeSearch />
            ) : (
              <Composer
                variant="home"
                sessionId={chief.home_session_id}
                newTask={{ agentId: chief.id }}
                onSent={(created) => navigate(created ? `/sessions/${encodeURIComponent(created)}` : thread)}
                onDraftApplied={() => {
                  setMode("ask");
                  if (shared) setShareDraft(null);
                  else setPicked(null);
                }}
                draft={draft}
              />
            )}
          </div>
          <SuggestedTasks suggestions={HOME_SUGGESTIONS} onPick={pick} />
          <RecentTasks sessionId={chief.home_session_id} to={thread} />
        </div>
      </div>
    </main>
  );
}
