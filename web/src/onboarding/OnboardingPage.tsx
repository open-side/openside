import { useCallback, useEffect, useState, type ReactNode } from "react";
import { useNavigate, useSearchParams } from "react-router";
import { ApiError } from "../api/client";
import { useClient } from "../context";
import { t } from "../i18n";
import { useToast } from "../ui";
import "../pages/pages.css";
import { dismissGuide } from "./dismissed";
import { DoneStep } from "./DoneStep";
import { BrowserImportStep } from "./ImportStep";
import "./onboarding.css";
import { PasswordsStep } from "./PasswordsStep";
import { SubscriptionStep } from "./SubscriptionStep";
import { EMPTY_SUMMARY, type StepId, type StepNav, type Summary } from "./summary";
import { WelcomeStep } from "./WelcomeStep";

/** Step order, as in Aside's first-run window; `?step=` names one of them. */
export const STEPS: readonly StepId[] = ["welcome", "import", "passwords", "subscription", "done"];

const BODY: Record<StepId, (nav: StepNav) => ReactNode> = {
  welcome: (nav) => <WelcomeStep {...nav} />,
  import: (nav) => <BrowserImportStep {...nav} />,
  passwords: (nav) => <PasswordsStep {...nav} />,
  subscription: (nav) => <SubscriptionStep {...nav} />,
  done: (nav) => <DoneStep {...nav} />,
};

function parse(value: string | null): number {
  const index = STEPS.findIndex((step) => step === value);
  return index < 0 ? 0 : index;
}

/** `/welcome`: the first-run guide. The step lives in `?step=` so Back and reloads keep their place. */
export function OnboardingPage() {
  const client = useClient();
  const navigate = useNavigate();
  const toast = useToast();
  const [params, setParams] = useSearchParams();
  const raw = params.get("step");
  const index = parse(raw);
  const step = STEPS[index]!;
  // An unknown or removed step (or an explicit `welcome`) shows the welcome; drop it from the URL so Back and
  // bookmarks see the canonical `/welcome`, replacing the entry rather than adding one.
  const stale = raw !== null && index === 0;
  useEffect(() => {
    if (stale) setParams({}, { replace: true });
  }, [stale, setParams]);
  const [summary, setSummary] = useState<Summary>(EMPTY_SUMMARY);
  const [leaving, setLeaving] = useState(false);

  const update = useCallback((patch: Partial<Summary>) => setSummary((current) => ({ ...current, ...patch })), []);
  const show = (id: StepId) => setParams(id === STEPS[0] ? {} : { step: id });

  /**
   * Ends the guide. Only a guide the daemon confirms as pending is written: reopening it from Settings must not turn
   * "completed" into "skipped", and an unreadable state is not proof that it is still pending. A failed read or write
   * dismisses the guide locally instead.
   */
  async function leave(outcome: "completed" | "skipped", to = "/") {
    setLeaving(true);
    try {
      const current = await client.onboarding();
      if (current.state === "pending") await client.setOnboarding(outcome);
    } catch (cause) {
      // Another device recorded its outcome between our read and write: the daemon already says the guide ended.
      const alreadySet = cause instanceof ApiError && cause.status === 409 && cause.code === "onboarding_already_set";
      if (!alreadySet) {
        dismissGuide();
        if (!(cause instanceof ApiError && cause.status === 403)) toast.notify(t("onboarding.page.saveFailed"), "error");
      }
    }
    navigate(to, { replace: true });
  }

  const nav: StepNav = {
    next: () => show(STEPS[Math.min(index + 1, STEPS.length - 1)]!),
    back: () => show(STEPS[Math.max(index - 1, 0)]!),
    leave,
    leaving,
    summary,
    update,
  };

  return (
    <main className="onb" aria-labelledby="onb-title">
      {BODY[step](nav)}
    </main>
  );
}
