import { useState, type ComponentType } from "react";
import { Link, useNavigate, type Location } from "react-router";
import { ChannelsPanel } from "../channels/ChannelsPanel";
import { ModelsPanel } from "../models/ModelsPanel";
import { RemoteAccess } from "../remote/RemoteAccess";
import { Dialog, ICON_STROKE } from "../ui";
import { ConnectionsPage } from "./ConnectionsPage";
import { AgentsPage } from "./AgentsPage";
import { ConfigPage } from "./ConfigPage";
import { DevicesPage } from "./DevicesPage";
import { DiagnosticsPage } from "./DiagnosticsPage";
import { backgroundOf, manageTarget, WORKSPACE_SECTIONS, type WorkspacePath } from "./manageRoutes";
import { MemoryPage } from "./MemoryPage";
import { PolicyPage } from "./PolicyPage";
import { RoutinesPage } from "./RoutinesPage";
import { SettingsPage } from "./SettingsPage";
import { SkillsPage } from "./SkillsPage";
import { SyncPage } from "./SyncPage";
import { TeamPage } from "./TeamPage";
import { VaultPage } from "./VaultPage";
import "./pages.css";

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

/**
 * Settings and Workspace open as dialogs (bottom sheets on phones) over the screen they were
 * opened from, as in aside-relay; their routes stay deep-linkable. `location` is the real
 * location: the routes underneath render the background one.
 */
export function ManageDialogs({ location, onSignOut }: { location: Location; onSignOut: () => void }) {
  const navigate = useNavigate();
  const target = manageTarget(location.pathname);
  // The last dialog route stays rendered while its dialog animates out after Close.
  const [shownPath, setShownPath] = useState(location.pathname);
  if (target && shownPath !== location.pathname) setShownPath(location.pathname);
  const shown = manageTarget(shownPath);
  const section = shown?.kind === "workspace" ? WORKSPACE_SECTIONS.find((item) => item.path === shown.section) : undefined;
  const Body = section ? SECTION_BODY[section.path] : undefined;

  // Moving between Settings, Workspace and sections replaces the entry, so one Back (or Close)
  // returns to the screen underneath.
  const go = (path: string) => navigate(path, { replace: true, state: location.state });
  const close = () => {
    if (backgroundOf(location)) navigate(-1);
    else navigate("/", { replace: true });
  };

  return (
    <>
      <Dialog open={target?.kind === "settings"} onClose={close} title="Settings">
        <SettingsPage onModels={() => go("/models")} onChannels={() => go("/channels")} onRemote={() => go("/remote")} onWorkspace={() => go("/workspace")} onGuide={() => navigate("/welcome", { replace: true })} onSignOut={onSignOut} />
      </Dialog>
      <Dialog
        open={target?.kind === "models"}
        onClose={close}
        size="wide"
        title="Models & accounts"
        description="Connect providers, choose the default model and see what each one makes available."
        back={{ label: "Back to settings", onClick: () => go("/settings") }}
      >
        <ModelsPanel />
      </Dialog>
      <Dialog
        open={target?.kind === "channels"}
        onClose={close}
        size="wide"
        title="Channels"
        description="Chat accounts your teammates answer on. Each account stays separate."
        back={{ label: "Back to settings", onClick: () => go("/settings") }}
      >
        <ChannelsPanel onRemote={() => go("/remote")} />
      </Dialog>
      <Dialog
        open={target?.kind === "remote"}
        onClose={close}
        size="wide"
        title="Remote access"
        description="Reach openside from your phone, and connect it with a QR code."
        back={{ label: "Back to settings", onClick: () => go("/settings") }}
      >
        <RemoteAccess />
      </Dialog>
      <Dialog
        open={target?.kind === "workspace"}
        onClose={close}
        size="wide"
        title={section?.title ?? "Workspace"}
        description={section?.description ?? "Stored by the openside daemon on your computer."}
        back={section ? { label: "Back to workspace", onClick: () => go("/workspace") } : undefined}
      >
        {Body ? (
          <Body key={section?.path} />
        ) : (
          <nav className="workspace-grid" aria-label="Workspace sections">
            {WORKSPACE_SECTIONS.map(({ path, title, description, Icon }) => (
              <Link key={path} to={path} replace state={location.state} className="workspace-tile">
                <span className="tile-icon">
                  <Icon size={20} strokeWidth={ICON_STROKE} aria-hidden="true" />
                </span>
                <span className="tile-copy">
                  <strong>{title}</strong>
                  <span>{description}</span>
                </span>
              </Link>
            ))}
          </nav>
        )}
      </Dialog>
    </>
  );
}
