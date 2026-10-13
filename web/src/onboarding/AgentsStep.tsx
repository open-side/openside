import { useCallback } from "react";
import type { ExternalAgent } from "../api/types";
import { AgentsPanel } from "../pages/AgentsPage";
import { CliHint } from "../pages/CliHint";
import type { StepProps } from "./summary";

export function AgentsStep({ update }: StepProps) {
  const onLoaded = useCallback(
    (agents: ExternalAgent[]) => update({ skills: agents.filter((agent) => agent.skill_installed).map((agent) => agent.label) }),
    [update],
  );
  return (
    <>
      <AgentsPanel onLoaded={onLoaded} />
      <p className="onb-note">An agent that is not listed can be added with the Add agent button, then installed here.</p>
      <CliHint command="openside skills install <agent>" />
    </>
  );
}
