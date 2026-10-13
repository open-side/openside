import { Check, Minus } from "lucide-react";
import { useCallback, useEffect, useRef, useState, type ReactNode } from "react";
import { useNavigate, useSearchParams } from "react-router";
import { ApiError } from "../api/client";
import { useClient } from "../context";
import { BrandMark, Button, cx, useToast } from "../ui";
import "../pages/pages.css";
import { AgentsStep } from "./AgentsStep";
import { AssistantStep } from "./AssistantStep";
import { dismissGuide } from "./dismissed";
import { ImportStep } from "./ImportStep";
import { ModelsStep } from "./ModelsStep";
import "./onboarding.css";
import { PairStep } from "./PairStep";
import { EMPTY_SUMMARY, type StepProps, type Summary } from "./summary";

export const STEPS = [
  {
    id: "import",
    label: "Browser",
    title: "Bring your browser with you",
    lead: "Start from where you already are. openside finds the browsers on this computer and brings over your sign-ins, passwords, bookmarks and history, so your agent can work in sites you’re already signed in to.",
  },
  { id: "models", label: "Models", title: "Connect a model", lead: "openside needs a model to answer. Add an API key, or use a Claude Code or Codex subscription already on this computer; keys are stored encrypted in the vault." },
  { id: "agents", label: "Agents", title: "Connect your agents", lead: "Install the openside browser skill so the agents on this computer hand browser work to openside. It adds one folder, openside-browser, to each agent’s skills." },
  { id: "pair", label: "Phone", title: "Pair your phone", lead: "Approve requests and watch the agent work from your phone." },
  { id: "assistant", label: "Assistant", title: "Name your assistant", lead: "This is who you talk to. They take your requests, bring in teammates for work that keeps coming back, and come to you when something needs your call." },
  { id: "done", label: "Done", title: "You’re all set", lead: "You can open this guide again any time from Settings." },
] as const;

export type StepId = (typeof STEPS)[number]["id"];

const BODY: Partial<Record<StepId, (props: StepProps) => ReactNode>> = {
  import: (props) => <ImportStep {...props} />,
  models: (props) => <ModelsStep {...props} />,
  agents: (props) => <AgentsStep {...props} />,
  pair: (props) => <PairStep {...props} />,
  assistant: (props) => <AssistantStep {...props} />,
};

function Recap({ summary }: { summary: Summary }) {
  const rows: { label: string; value: string | null }[] = [
    { label: "Browser data", value: summary.imported ? `Brought over: ${summary.imported}` : null },
    { label: "Models", value: summary.models ? `${summary.models} ready` : null },
    { label: "Agents", value: summary.skills.length ? `Browser skill for ${summary.skills.join(", ")}` : null },
    { label: "Phone", value: summary.phone ? `${summary.phone} paired` : null },
    { label: "Assistant", value: summary.assistant },
  ];
  return (
    <ul className="onb-recap" aria-label="Summary">
      {rows.map(({ label, value }) => (
        <li key={label} data-done={value ? "" : undefined}>
          <span className="onb-recap-mark" aria-hidden="true">
            {value ? <Check size={14} strokeWidth={2.4} /> : <Minus size={14} strokeWidth={2.4} />}
          </span>
          <span>
            <strong>{label}</strong>
            <span>{value ?? "Skipped"}</span>
          </span>
        </li>
      ))}
    </ul>
  );
}

function parse(value: string | null): number {
  const index = STEPS.findIndex((step) => step.id === value);
  return index < 0 ? 0 : index;
}

/** `/welcome`: the first-run guide. The step lives in `?step=` so Back and reloads keep their place. */
export function OnboardingPage() {
  const client = useClient();
  const navigate = useNavigate();
  const toast = useToast();
  const [params, setParams] = useSearchParams();
  const index = parse(params.get("step"));
  const step = STEPS[index]!;
  const [summary, setSummary] = useState<Summary>(EMPTY_SUMMARY);
  const [leaving, setLeaving] = useState(false);
  const heading = useRef<HTMLHeadingElement>(null);

  const update = useCallback((patch: Partial<Summary>) => setSummary((current) => ({ ...current, ...patch })), []);
  const show = (id: StepId) => setParams(id === STEPS[0].id ? {} : { step: id });

  useEffect(() => {
    heading.current?.focus({ preventScroll: true });
  }, [step.id]);

  /** Ends the guide. Only a pending guide is written: reopening it from Settings must not turn "completed" into "skipped". */
  async function leave(outcome: "completed" | "skipped") {
    setLeaving(true);
    try {
      const current = await client.onboarding().catch(() => null);
      if (current === null || current.state === "pending") await client.setOnboarding(outcome);
    } catch (cause) {
      dismissGuide();
      if (!(cause instanceof ApiError && cause.status === 403)) toast.notify("Could not save that setup is finished. It may open again.", "error");
    }
    navigate("/", { replace: true });
  }

  const Body = BODY[step.id];
  const first = index === 0;
  const last = step.id === "done";
  const complete =
    (step.id === "import" && summary.imported !== null) ||
    (step.id === "models" && summary.models !== null) ||
    (step.id === "agents" && summary.skills.length > 0) ||
    (step.id === "pair" && summary.phone !== null) ||
    (step.id === "assistant" && summary.assistant !== null);
  const next = STEPS[index + 1];

  return (
    <main className="onb" aria-labelledby="onb-title">
      <div className="onb-card panel">
        <header className="onb-header">
          <span className="onb-brand">
            <BrandMark size={22} />
            openside
          </span>
          {last ? null : (
            <Button variant="ghost" size="compact" disabled={leaving} onClick={() => void leave("skipped")}>
              Skip setup
            </Button>
          )}
        </header>
        <ol className="onb-progress" aria-label={`Step ${index + 1} of ${STEPS.length}: ${step.label}`}>
          {STEPS.map((item, position) => (
            <li key={item.id} data-state={position < index ? "past" : position === index ? "current" : "future"} aria-current={position === index ? "step" : undefined}>
              <span className="sr-only">{item.label}</span>
            </li>
          ))}
        </ol>
        <section className="onb-body" key={step.id}>
          <h1 id="onb-title" tabIndex={-1} ref={heading}>
            {step.title}
          </h1>
          <p className="onb-lead">{step.lead}</p>
          {Body?.({ update })}
          {last ? <Recap summary={summary} /> : null}
        </section>
        <footer className={cx("onb-footer", first && "onb-footer--solo")}>
          {first ? null : (
            <Button variant="ghost" disabled={leaving} onClick={() => show(STEPS[index - 1]!.id)}>
              Back
            </Button>
          )}
          {last ? (
            <Button variant="primary" disabled={leaving} onClick={() => void leave("completed")}>
              Open openside
            </Button>
          ) : (
            <Button variant={complete ? "primary" : first ? "ghost" : "secondary"} disabled={leaving} onClick={() => show(next!.id)}>
              {complete ? "Continue" : first ? "Skip for now" : "Skip this step"}
            </Button>
          )}
        </footer>
      </div>
    </main>
  );
}
