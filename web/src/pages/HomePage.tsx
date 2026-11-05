import { useNavigate } from "react-router";
import { AppHeader } from "../components/AppHeader";
import { Composer } from "../components/Composer";
import { ModelButton } from "../components/ModelSheet";
import { RecentTasks } from "../components/RecentTasks";
import { people, useTeam } from "../hooks/useAgents";
import { setShareDraft, useShareDraft } from "../pwa/share";
import { Spinner } from "../ui";

/**
 * `/`: hand the assistant a task. The message goes into the assistant's one continuous thread,
 * which opens once it is accepted (or parked in the outbox); the pending bubble carries over.
 */
export function HomePage() {
  const navigate = useNavigate();
  const team = useTeam();
  const chief = people(team.agents ?? []).chief;
  const draft = useShareDraft();

  if (!chief) {
    return (
      <main className="home">
        <AppHeader newTask={false} />
        <div className="home-loading">{team.offline ? null : <Spinner label="Loading" />}</div>
      </main>
    );
  }
  const thread = `/agents/${encodeURIComponent(chief.id)}`;
  return (
    <main className="home">
      <AppHeader newTask={false} />
      <div className="home-scroll thin-scroll">
        <div className="home-column">
          <h1 className="home-title">What should we do?</h1>
          <Composer
            variant="home"
            sessionId={chief.home_session_id}
            onSent={() => navigate(thread)}
            draft={draft}
            onDraftApplied={() => setShareDraft(null)}
          />
          <div className="home-chips">
            <ModelButton agent={chief} />
          </div>
          <RecentTasks sessionId={chief.home_session_id} to={thread} />
        </div>
      </div>
    </main>
  );
}
