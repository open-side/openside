import { Bot } from "lucide-react";
import { useCallback, useEffect, useState } from "react";
import { ApiError } from "../api/client";
import { useClient } from "../context";
import { errorMessage, useResource } from "../hooks/useResource";
import { ErrorNote, ResourceCard, ResourceView } from "../pages/blocks";
import { Button } from "../ui";
import type { StepProps } from "./summary";

export function AgentsStep({ update }: StepProps) {
  const client = useClient();
  const load = useCallback(() => client.externalAgents(), [client]);
  const agents = useResource(load);
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const { data, reload } = agents;

  useEffect(() => {
    if (data) update({ skills: data.agents.filter((agent) => agent.skill_installed).map((agent) => agent.label) });
  }, [data, update]);

  async function install(id: string) {
    setBusy(id);
    setError(null);
    try {
      await client.installAgentSkill(id);
    } catch (cause) {
      // Installed from the terminal while the guide was open: the list below is the truth.
      if (!(cause instanceof ApiError && cause.code === "exists")) setError(errorMessage(cause));
    } finally {
      setBusy(null);
    }
    await reload();
  }

  return (
    <ResourceView resource={agents} what="coding agents">
      {({ agents: list }) => (
        <>
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
                        Skills folder <code className="onb-path">{breakable(agent.skills_dir)}</code>
                      </>
                    ) : (
                      "openside does not know where this agent keeps skills. Set it under [skills.agents] in config.toml."
                    )
                  }
                  trailing={
                    agent.skill_installed || !agent.skills_dir ? undefined : (
                      <Button
                        size="compact"
                        variant={agent.detected ? "primary" : "secondary"}
                        disabled={busy !== null}
                        aria-label={`Install the browser skill for ${agent.label}`}
                        onClick={() => void install(agent.id)}
                      >
                        {busy === agent.id ? "Installing…" : agent.detected ? "Install" : "Install anyway"}
                      </Button>
                    )
                  }
                />
              </li>
            ))}
          </ul>
          {error ? <ErrorNote>{error}</ErrorNote> : null}
          <p className="onb-note">
            An agent that is not listed can be set up later with <code>openside skills install &lt;agent&gt;</code>.
          </p>
        </>
      )}
    </ResourceView>
  );
}

// A phone-width row may wrap only between folders, never inside a name.
function breakable(path: string) {
  return path.split("/").flatMap((part, index) => (index === 0 ? [part] : [<wbr key={index} />, `/${part}`]));
}
