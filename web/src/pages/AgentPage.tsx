import { Ellipsis, EyeOff, Eye, UserMinus, UserRound } from "lucide-react";
import { useCallback, useState } from "react";
import { Navigate, useParams, useSearchParams } from "react-router";
import { ApiError } from "../api/client";
import type { Agent, AgentSummary } from "../api/types";
import { AgentProfileSheet } from "../components/AgentProfileSheet";
import { AppHeader } from "../components/AppHeader";
import { Avatar } from "../components/Avatar";
import { useTeamActions } from "../components/TeamActions";
import { Starters } from "../components/Starters";
import { useClient } from "../context";
import { useTeam } from "../hooks/useAgents";
import { setShareDraft, useShareDraft } from "../pwa/share";
import { seen } from "../state/seen";
import { Button, ICON_STROKE, IconButton, MenuItem, MenuSeparator, PopoverMenu, Spinner, useToast } from "../ui";
import { ThreadView } from "./ThreadView";

/** `/agents/:id`: the one continuous thread with a person on the team. */
export function AgentPage() {
  const { id = "" } = useParams();
  const team = useTeam();
  const agent = team.agents?.find((candidate) => candidate.id === id);
  const [params, setParams] = useSearchParams();
  const profileOpen = params.has("profile");
  const { retire } = useTeamActions();
  const client = useClient();
  const toast = useToast();
  const isChief = agent?.kind === "chief";
  const draft = useShareDraft();
  const onSeen = useCallback((seq: number) => seen.mark(id, seq), [id]);

  const setProfile = (open: boolean) =>
    setParams(
      (current) => {
        if (open) current.set("profile", "");
        else current.delete("profile");
        return current;
      },
      { replace: !open },
    );

  if (!agent) {
    if (team.agents === null)
      return (
        <main className="conversation agent-loading">
          <AppHeader title="…" />
          <div className="empty-chat">
            <Spinner label="Loading" />
          </div>
        </main>
      );
    return <Navigate to="/" replace />;
  }

  const setHidden = async (hidden: boolean) => {
    try {
      await client.updateAgent(agent.id, { hidden });
      team.refresh();
    } catch (cause) {
      toast.notify(cause instanceof ApiError ? cause.message : "Could not update the sidebar", "error");
    }
  };

  return (
    <>
      <ThreadView
        key={agent.home_session_id}
        sessionId={agent.home_session_id}
        addressee={agent.name}
        onSeen={onSeen}
        front={isChief}
        readOnly={agent.status === "retired" ? <RetiredNotice agent={agent} /> : undefined}
        empty={isChief ? (pick) => <ChiefWelcome agent={agent} onPick={pick} /> : undefined}
        draft={isChief ? draft : undefined}
        onDraftApplied={isChief ? () => setShareDraft(null) : undefined}
        header={({ running, waiting }) => (
          <AppHeader
            heading={
              <button type="button" className="app-header-heading person-heading" onClick={() => setProfile(true)} aria-label={`${agent.name}'s profile`}>
                <Avatar chief={agent.kind === "chief"} initials={agent.initials} color={agent.color} size={32} />
                <span className="person-heading-text">
                  <span className="app-title">{agent.name}</span>
                  <span className="app-subtitle">{waiting || agent.needs_you ? "Needs you" : running ? agent.activity.trim() || "Working…" : agent.persona.role}</span>
                </span>
              </button>
            }
            actions={
              <PopoverMenu
                label={`Options for ${agent.name}`}
                trigger={
                  <IconButton label={`Options for ${agent.name}`}>
                    <Ellipsis size={20} strokeWidth={ICON_STROKE} aria-hidden="true" />
                  </IconButton>
                }
              >
                <MenuItem icon={<UserRound size={16} strokeWidth={ICON_STROKE} />} onClick={() => setProfile(true)}>
                  Profile
                </MenuItem>
                {isChief || agent.status === "retired" ? null : (
                  <>
                    <MenuItem
                      icon={agent.hidden ? <Eye size={16} strokeWidth={ICON_STROKE} /> : <EyeOff size={16} strokeWidth={ICON_STROKE} />}
                      onClick={() => void setHidden(!agent.hidden)}
                    >
                      {agent.hidden ? "Show in sidebar" : "Hide from sidebar"}
                    </MenuItem>
                    <MenuSeparator />
                    <MenuItem danger icon={<UserMinus size={16} strokeWidth={ICON_STROKE} />} onClick={() => retire(agent.id)}>
                      Retire {agent.name}…
                    </MenuItem>
                  </>
                )}
              </PopoverMenu>
            }
          />
        )}
      />
      <AgentProfileSheet
        agent={agent}
        open={profileOpen}
        onClose={() => setProfile(false)}
        onRetire={() => {
          setProfile(false);
          retire(agent.id);
        }}
      />
    </>
  );
}

/** The chief's empty thread on first run: who it is, and a few things to ask. */
function ChiefWelcome({ agent, onPick }: { agent: AgentSummary; onPick: (text: string) => void }) {
  return (
    <div className="empty-chat chief-welcome">
      <Avatar chief initials={agent.initials} color={agent.color} size={64} />
      <h1>Hi, I’m {agent.name}</h1>
      <p>Tell me what to do on the web. I’ll ask before anything important, and bring in a teammate when work keeps coming back.</p>
      <Starters onPick={onPick} />
    </div>
  );
}

/** Replaces the composer in a retired teammate's thread. */
export function RetiredNotice({ agent }: { agent: Agent }) {
  const client = useClient();
  const team = useTeam();
  const toast = useToast();
  const [busy, setBusy] = useState(false);
  const restore = async () => {
    setBusy(true);
    try {
      await client.restoreAgent(agent.id);
      team.refresh();
    } catch (cause) {
      toast.notify(cause instanceof ApiError ? cause.message : `Could not restore ${agent.name}`, "error");
    } finally {
      setBusy(false);
    }
  };
  return (
    <div className="composer-dock">
      <div className="composer-readonly retired-notice" role="note">
        <span>{agent.name} is retired. This thread is kept for reference.</span>
        <Button size="compact" disabled={busy} onClick={() => void restore()}>
          Restore {agent.name}
        </Button>
      </div>
    </div>
  );
}
