import { Ellipsis } from "lucide-react";
import { Navigate, useNavigate, useParams } from "react-router";
import { AppHeader } from "../components/AppHeader";
import { SessionMenu } from "../components/SessionMenu";
import { displayTitle } from "../components/toolStatus";
import { useTeam } from "../hooks/useAgents";
import { useShell } from "../shell";
import { ICON_STROKE, IconButton } from "../ui";
import { RetiredNotice } from "./AgentPage";
import { ThreadView } from "./ThreadView";

/**
 * `/sessions/:id`: a conversation from History. An active teammate's home thread opens as that
 * person instead, so there is one address for each continuous thread.
 */
export function SessionPage() {
  const { id = "" } = useParams();
  const shell = useShell();
  const navigate = useNavigate();
  const owner = useTeam().agents?.find((agent) => agent.home_session_id === id);
  if (owner?.status === "active") return <Navigate to={`/agents/${encodeURIComponent(owner.id)}`} replace />;

  return (
    <ThreadView
      key={id}
      sessionId={id}
      readOnly={owner ? <RetiredNotice agent={owner} /> : undefined}
      header={({ view, running, waiting }) => (
        <AppHeader
          title={displayTitle(view.title) || "…"}
          titleClassName="session-heading"
          // Status only: the model id is a provider identifier, not something to read in the title.
          subtitle={waiting ? "Needs you" : running ? "Working…" : view.status === "stopped" ? "Stopped" : undefined}
          actions={
            <SessionMenu
              session={{ id, title: view.title, pinned: view.pinned, archived: view.archived }}
              label="Conversation options"
              onChanged={shell.refreshSessions}
              onDeleted={() => navigate("/history", { replace: true })}
              trigger={
                <IconButton label="Conversation options">
                  <Ellipsis size={20} strokeWidth={ICON_STROKE} aria-hidden="true" />
                </IconButton>
              }
            />
          }
        />
      )}
    />
  );
}
