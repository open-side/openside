import { Bot, Check, Globe, KeyRound, Minus, Smartphone } from "lucide-react";
import { useCallback, useEffect, useRef, useState, type ReactNode } from "react";
import { useNavigate, useSearchParams } from "react-router";
import { ApiError } from "../api/client";
import { useClient } from "../context";
import { BrandMark, Button, cx, ICON_STROKE, useToast } from "../ui";
import "../pages/pages.css";
import { AgentsStep } from "./AgentsStep";
import { dismissGuide } from "./dismissed";
import { ImportStep } from "./ImportStep";
import "./onboarding.css";
import { PairStep } from "./PairStep";
import { EMPTY_SUMMARY, type StepProps, type Summary } from "./summary";
import { VaultStep } from "./VaultStep";

export const STEPS = [
  { id: "welcome", label: "Welcome", title: "Welcome to openside", lead: "Your agent works in a real browser on this computer. A few quick steps connect it to what you already use." },
  { id: "agents", label: "Agents", title: "Connect your agents", lead: "Install the openside browser skill so the agents on this computer hand browser work to openside. It adds one folder, openside-browser, to each agent’s skills." },
  { id: "import", label: "Browser data", title: "Bring your browser data", lead: "Copy history, bookmarks and cookies from a browser on this computer, so the agent starts signed in where you are. You see the counts before anything is written." },
  { id: "vault", label: "Vault", title: "Keep logins in the vault", lead: "The vault stores logins and one-time-code keys encrypted on this computer. Login values are left out of the agent’s tool results, but scripts the agent runs on a page can read its filled fields." },
  { id: "pair", label: "Phone", title: "Pair your phone", lead: "Approve requests and watch the agent work from your phone." },
  { id: "done", label: "Done", title: "You’re all set", lead: "You can open this guide again any time from Settings." },
] as const;

export type StepId = (typeof STEPS)[number]["id"];

const BODY: Partial<Record<StepId, (props: StepProps) => ReactNode>> = {
  agents: (props) => <AgentsStep {...props} />,
  import: (props) => <ImportStep {...props} />,
  vault: (props) => <VaultStep {...props} />,
  pair: (props) => <PairStep {...props} />,
};

const OVERVIEW = [
  { icon: Bot, text: "Connect the coding agents on this computer" },
  { icon: Globe, text: "Bring in your browser’s history, bookmarks and cookies" },
  { icon: KeyRound, text: "Keep logins in the vault" },
  { icon: Smartphone, text: "Pair your phone" },
] as const;

function Welcome() {
  return (
    <ul className="onb-overview">
      {OVERVIEW.map(({ icon: Icon, text }) => (
        <li key={text}>
          <span className="onb-overview-icon" aria-hidden="true">
            <Icon size={18} strokeWidth={ICON_STROKE} />
          </span>
          {text}
        </li>
      ))}
    </ul>
  );
}

function Recap({ summary }: { summary: Summary }) {
  const rows: { label: string; value: string | null }[] = [
    { label: "Agents", value: summary.skills.length ? `Browser skill for ${summary.skills.join(", ")}` : null },
    { label: "Browser data", value: summary.imported ? `Imported from ${summary.imported}` : null },
    { label: "Vault", value: summary.vaultReady ? "Set up" : null },
    { label: "Phone", value: summary.phone ? `${summary.phone} paired` : null },
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
  const show = (id: StepId) => setParams(id === "welcome" ? {} : { step: id });

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
    (step.id === "agents" && summary.skills.length > 0) ||
    (step.id === "import" && summary.imported !== null) ||
    (step.id === "vault" && summary.vaultReady) ||
    (step.id === "pair" && summary.phone !== null);
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
          {first ? <Welcome /> : null}
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
            <Button variant={first || complete ? "primary" : "secondary"} disabled={leaving} onClick={() => show(next!.id)}>
              {first ? "Get started" : complete ? "Continue" : "Skip this step"}
            </Button>
          )}
        </footer>
      </div>
    </main>
  );
}
