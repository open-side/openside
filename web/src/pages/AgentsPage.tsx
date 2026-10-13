import { Bot, FolderCog, Trash2 } from "lucide-react";
import { useCallback, useEffect, useState } from "react";
import { ApiError } from "../api/client";
import type { ExternalAgent } from "../api/types";
import { useClient } from "../context";
import { errorMessage, useResource } from "../hooks/useResource";
import { Button, ICON_STROKE, MenuItem, useToast } from "../ui";
import { AgentFolderDialog } from "./AgentFolderDialog";
import { AddButton, ErrorNote, ResourceCard, ResourceView, RowMenu, SectionList, useConfirm, useDialogState } from "./blocks";

// A phone-width row may wrap only between folders, never inside a name.
function breakable(path: string) {
  return path.split("/").flatMap((part, index) => (index === 0 ? [part] : [<wbr key={index} />, `/${part}`]));
}

/**
 * The coding agents openside can install its browser skill for: the built-in ones it finds on
 * this computer, plus any agent the person adds by its skills folder. Shared by Workspace and the
 * first-run guide.
 */
export function AgentsPanel({ onLoaded }: { onLoaded?: (agents: ExternalAgent[]) => void }) {
  const client = useClient();
  const toast = useToast();
  const load = useCallback(() => client.externalAgents(), [client]);
  const agents = useResource(load);
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const folder = useDialogState<{ agent?: ExternalAgent }>();
  const [confirm, ask] = useConfirm();
  const { data, reload } = agents;

  useEffect(() => {
    if (data) onLoaded?.(data.agents);
  }, [data, onLoaded]);

  async function install(id: string) {
    setBusy(id);
    setError(null);
    try {
      await client.installAgentSkill(id);
    } catch (cause) {
      // Installed by another route while the list was open: the list below is the truth.
      if (!(cause instanceof ApiError && cause.code === "exists")) setError(errorMessage(cause));
    } finally {
      setBusy(null);
    }
    await reload();
  }

  return (
    <ResourceView resource={agents} what="coding agents">
      {({ agents: list }) => (
        <SectionList
          label="Coding agents"
          summary={`${list.filter((agent) => agent.detected).length} of ${list.length} found on this computer`}
          actions={<AddButton onClick={() => folder.show({})}>Add agent</AddButton>}
        >
          <ul className="resource-list" aria-label="Agents">
            {list.map((agent) => (
              <li key={agent.id}>
                <ResourceCard
                  icon={Bot}
                  title={agent.label}
                  badge={agent.skill_installed ? "Skill installed" : agent.detected ? "Found" : "Not found"}
                  badgeTone={agent.skill_installed ? "success" : "neutral"}
                  detail={
                    agent.skills_dir ? (
                      <>
                        Skills folder <code className="agent-path">{breakable(agent.skills_dir)}</code>
                      </>
                    ) : (
                      "openside does not know where this agent keeps its skills. Add it as another agent with its folder."
                    )
                  }
                  trailing={
                    <>
                      {agent.skill_installed || !agent.skills_dir ? null : (
                        <Button
                          size="compact"
                          variant={agent.detected ? "primary" : "secondary"}
                          disabled={busy !== null}
                          aria-label={`Install the browser skill for ${agent.label}`}
                          onClick={() => void install(agent.id)}
                        >
                          {busy === agent.id ? "Installing…" : agent.detected ? "Install" : "Install anyway"}
                        </Button>
                      )}
                      {agent.custom ? (
                        <RowMenu name={agent.label}>
                          <MenuItem icon={<FolderCog size={18} strokeWidth={ICON_STROKE} />} onClick={() => folder.show({ agent })}>
                            Change skills folder
                          </MenuItem>
                          <MenuItem
                            danger
                            icon={<Trash2 size={18} strokeWidth={ICON_STROKE} />}
                            onClick={() =>
                              ask({
                                title: `Forget ${agent.label}?`,
                                description: "openside stops offering to install its skill. Files already in its skills folder stay where they are.",
                                confirmLabel: "Forget",
                                onConfirm: () =>
                                  void client.deleteExternalAgent(agent.id).then(
                                    () => reload(),
                                    (cause: unknown) => setError(errorMessage(cause)),
                                  ),
                              })
                            }
                          >
                            Forget agent
                          </MenuItem>
                        </RowMenu>
                      ) : null}
                    </>
                  }
                />
              </li>
            ))}
          </ul>
          {error ? <ErrorNote>{error}</ErrorNote> : null}
          {folder.value ? (
            <AgentFolderDialog
              key={folder.value.agent?.id ?? "new"}
              open={folder.open}
              agent={folder.value.agent}
              onClose={folder.close}
              onSaved={(id) => {
                folder.close();
                toast.notify(folder.value?.agent ? "Skills folder saved." : `${id} added.`);
                void reload();
              }}
            />
          ) : null}
          {confirm}
        </SectionList>
      )}
    </ResourceView>
  );
}

export function AgentsPage() {
  return (
    <div className="manage-body" data-section="agents">
      <p className="settings-text">
        openside can install its browser skill for the coding agents on this computer, so they can use the same browser. Add an agent that is not listed by telling openside where it keeps its skills.
      </p>
      <AgentsPanel />
    </div>
  );
}
