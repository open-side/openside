import { Ellipsis, Folder, Share } from "lucide-react";
import { useEffect, useState } from "react";
import { useNavigate, useParams } from "react-router";
import type { Project } from "../api/types";
import { AppHeader } from "../components/AppHeader";
import { AgentTabsToggle } from "../components/AgentTabsPanel";
import { useAgentTabsPanel } from "../components/agentTabsStore";
import { BrowserButton } from "../components/BrowserButton";
import { SessionMenu } from "../components/SessionMenu";
import { ShareDialog } from "../components/ShareDialog";
import { SplitPane, SplitToggle } from "../components/SplitPane";
import { SplitPaneContext, useSplitPane, useSplitPaneState } from "../components/splitPaneState";
import { displayTitle } from "../components/toolStatus";
import { useClient } from "../context";
import { t } from "../i18n";
import { useShell } from "../shell";
import type { SessionView } from "../state/session";
import { Button, ICON_STROKE, IconButton } from "../ui";
import { ThreadView } from "./ThreadView";

/**
 * The chat's project by name beside its title. The chat is read once for its project, since
 * `session.created` does not carry it; a later `session.folder_changed` in the view wins.
 */
function ProjectChip({ id, folder }: { id: string; folder: SessionView["folder"] }) {
  const client = useClient();
  const [read, setRead] = useState<string | null>(null);
  const [projects, setProjects] = useState<Project[]>([]);
  const projectId = folder ? folder.projectId : read;

  useEffect(() => {
    let live = true;
    // A daemon answering without a body (204) leaves the chat unread rather than breaking the header;
    // the chip is a hint, so a failed read just shows none.
    client
      .session(id)
      .then((session) => live && setRead(session?.project_id ?? null))
      .catch(() => undefined);
    return () => {
      live = false;
    };
  }, [client, id]);

  // Read again when the chat moves, so a project made from the composer a moment ago has its name.
  useEffect(() => {
    if (!projectId) return;
    let live = true;
    client
      .listProjects()
      .then((list) => live && setProjects(list?.projects ?? []))
      .catch(() => undefined);
    return () => {
      live = false;
    };
  }, [client, projectId]);

  const project = projectId ? projects.find((entry) => entry.id === projectId) : undefined;
  if (!project) return null;
  return (
    <span className="thread-header-project" title={project.root}>
      <Folder size={12} strokeWidth={ICON_STROKE} aria-hidden="true" />
      <span className="thread-header-project-name">{project.name}</span>
    </span>
  );
}

/** Title with its menu beside it, then Share, the browser and Agent Tabs controls and the split toggle. */
function ThreadHeader({ id, view, running }: { id: string; view: SessionView; running: boolean }) {
  const shell = useShell();
  const navigate = useNavigate();
  const panel = useAgentTabsPanel(id);
  const split = useSplitPane();
  const [sharing, setSharing] = useState(false);
  const title = displayTitle(view.title) || "…";
  return (
    <AppHeader
      heading={
        <div className="thread-header">
          <h1 className="app-title thread-header-title" title={title}>
            {title}
          </h1>
          <SessionMenu
            session={{ id, title: view.title, archived: view.archived }}
            label={t("timeline.session.options")}
            onChanged={shell.refreshSessions}
            onDeleted={() => navigate("/chats", { replace: true })}
            trigger={
              <IconButton label={t("timeline.session.options")} size="small" className="thread-header-more">
                <Ellipsis size={16} strokeWidth={ICON_STROKE} aria-hidden="true" />
              </IconButton>
            }
          />
          <ProjectChip id={id} folder={view.folder} />
        </div>
      }
      actions={
        <>
          <Button variant="ghost" size="small" className="thread-share" disabled={running} title={running ? t("timeline.session.shareRunning") : undefined} onClick={() => setSharing(true)}>
            <Share size={14} strokeWidth={ICON_STROKE} aria-hidden="true" />
            {t("timeline.session.share")}
          </Button>
          {/* Beside the Agent Tabs column its cards bring the browser back; the button is for when the column is closed. */}
          {panel.shown ? null : <BrowserButton sessionId={id} view={view} />}
          <AgentTabsToggle sessionId={id} view={view} />
          {/* While the pane is open, its own header closes it. */}
          {split?.open ? null : <SplitToggle />}
          {sharing ? <ShareDialog sessionId={id} onClose={() => setSharing(false)} /> : null}
        </>
      }
    />
  );
}

/** `/sessions/:id`: one chat, with a pane on the right for a file or a side chat. */
export function SessionPage() {
  const { id = "" } = useParams();
  return <SessionScreen key={id} id={id} />;
}

function SessionScreen({ id }: { id: string }) {
  const split = useSplitPaneState();
  return (
    <SplitPaneContext.Provider value={split}>
      <div className="thread-layout" data-split={split.open ? "open" : undefined}>
        <ThreadView sessionId={id} header={({ view, running }) => <ThreadHeader id={id} view={view} running={running} />} />
        {split.open ? <SplitPane sessionId={id} /> : null}
      </div>
    </SplitPaneContext.Provider>
  );
}
