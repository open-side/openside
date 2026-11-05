import { ChevronRight } from "lucide-react";
import { useState, type ComponentType } from "react";
import { Link, useNavigate, type Location } from "react-router";
import { ChannelsPanel } from "../channels/ChannelsPanel";
import { ModelsPanel } from "../models/ModelsPanel";
import { RemoteAccess } from "../remote/RemoteAccess";
import { cx, Dialog, ICON_STROKE } from "../ui";
import { ConnectionsPage } from "./ConnectionsPage";
import { AgentsPage } from "./AgentsPage";
import { ConfigPage } from "./ConfigPage";
import { DevicesPage } from "./DevicesPage";
import { DiagnosticsPage } from "./DiagnosticsPage";
import { backgroundOf, manageTarget, WORKSPACE_SECTIONS, type WorkspacePath } from "./manageRoutes";
import { MemoryPage } from "./MemoryPage";
import { PolicyPage } from "./PolicyPage";
import { RoutinesPage } from "./RoutinesPage";
import { SettingsNav, SettingsPage, type SettingsPane } from "./SettingsPage";
import { SkillsPage } from "./SkillsPage";
import { SyncPage } from "./SyncPage";
import { TeamPage } from "./TeamPage";
import { VaultPage } from "./VaultPage";
import "./pages.css";
import "./signin.css";

const SECTION_BODY: Record<WorkspacePath, ComponentType> = {
  "/team": TeamPage,
  "/skills": SkillsPage,
  "/routines": RoutinesPage,
  "/memory": MemoryPage,
  "/connections": ConnectionsPage,
  "/vault": VaultPage,
  "/devices": DevicesPage,
  "/agents": AgentsPage,
  "/sync": SyncPage,
  "/policy": PolicyPage,
  "/diagnostics": DiagnosticsPage,
  "/config": ConfigPage,
};

const SETTINGS_PANES: Record<SettingsPane, { title: string; description?: string }> = {
  settings: { title: "Settings" },
  models: { title: "Models & accounts", description: "Connect providers, choose the fast and reasoning models and see what each one makes available." },
  channels: { title: "Channels", description: "Chat accounts your teammates answer on. Each account stays separate." },
  remote: { title: "Remote access", description: "Reach openside from your phone, and connect it with a QR code." },
};

/**
 * The Workspace sections: the whole dialog on `/workspace`, the rail beside an open section.
 * `a.workspace-tile` with its `strong` title is how the e2e suites find a section.
 */
function WorkspaceNav({ current, state }: { current?: WorkspacePath; state: unknown }) {
  return (
    <nav className="settings-nav" aria-label="Workspace sections">
      {WORKSPACE_SECTIONS.map(({ path, title, description, Icon }) => (
        <Link key={path} to={path} replace state={state} className="settings-nav-row workspace-tile" aria-current={path === current ? "page" : undefined}>
          <Icon className="settings-nav-icon" size={18} strokeWidth={ICON_STROKE} aria-hidden="true" />
          <span className="settings-nav-copy">
            <strong>{title}</strong>
            <span className="settings-nav-detail">{description}</span>
          </span>
          <ChevronRight className="settings-nav-chevron" size={18} strokeWidth={ICON_STROKE} aria-hidden="true" />
        </Link>
      ))}
    </nav>
  );
}

/**
 * Settings and Workspace open as dialogs (bottom sheets on phones) over the screen they were
 * opened from, as in aside-relay; their routes stay deep-linkable. `location` is the real
 * location: the routes underneath render the background one.
 *
 * Each dialog swaps its pane in place: on wide screens a rail on the left lists the panes; on
 * phones the same list is a grouped list that drills into a pane with a back control.
 */
export function ManageDialogs({ location, onSignOut }: { location: Location; onSignOut: () => void }) {
  const navigate = useNavigate();
  const target = manageTarget(location.pathname);
  // The last route of each dialog stays rendered while that dialog animates out after Close.
  const [shown, setShown] = useState<{ settings: SettingsPane; section: WorkspacePath | null }>({ settings: "settings", section: null });
  const pane: SettingsPane | null = target && target.kind !== "workspace" ? target.kind : null;
  const section = target?.kind === "workspace" ? (target.section ?? null) : undefined;
  if (pane && pane !== shown.settings) setShown({ ...shown, settings: pane });
  else if (section !== undefined && section !== shown.section) setShown({ ...shown, section });

  // Moving between Settings, Workspace and sections replaces the entry, so one Back (or Close)
  // returns to the screen underneath.
  const go = (path: string) => navigate(path, { replace: true, state: location.state });
  const close = () => {
    if (backgroundOf(location)) navigate(-1);
    else navigate("/", { replace: true });
  };

  const settings = SETTINGS_PANES[shown.settings];
  const workspace = shown.section ? WORKSPACE_SECTIONS.find((item) => item.path === shown.section) : undefined;
  const Body = workspace ? SECTION_BODY[workspace.path] : undefined;

  return (
    <>
      <Dialog
        open={pane !== null}
        onClose={close}
        size="wide"
        className={cx("manage-dialog", "manage-dialog--rail")}
        title={settings.title}
        description={settings.description}
        back={shown.settings === "settings" ? undefined : { label: "Back to settings", onClick: () => go("/settings") }}
      >
        <div className="manage-layout" data-view={shown.settings === "settings" ? "index" : "pane"}>
          <SettingsNav current={shown.settings} onOpen={go} onGuide={() => navigate("/welcome", { replace: true })} />
          <div className="manage-pane">
            {shown.settings === "settings" ? (
              <SettingsPage onSignOut={onSignOut} />
            ) : shown.settings === "models" ? (
              <ModelsPanel />
            ) : shown.settings === "channels" ? (
              <ChannelsPanel onRemote={() => go("/remote")} />
            ) : (
              <RemoteAccess />
            )}
          </div>
        </div>
      </Dialog>
      <Dialog
        open={section !== undefined}
        onClose={close}
        size="wide"
        className={cx("manage-dialog", Body && "manage-dialog--rail")}
        title={workspace?.title ?? "Workspace"}
        description={workspace?.description ?? "Stored by the openside daemon on your computer."}
        back={workspace ? { label: "Back to workspace", onClick: () => go("/workspace") } : { label: "Back to settings", onClick: () => go("/settings") }}
      >
        {Body ? (
          <div className="manage-layout" data-view="pane">
            <WorkspaceNav current={workspace?.path} state={location.state} />
            <div className="manage-pane">
              <Body key={workspace?.path} />
            </div>
          </div>
        ) : (
          <WorkspaceNav state={location.state} />
        )}
      </Dialog>
    </>
  );
}
