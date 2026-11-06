import { t } from "../i18n";
import { Button } from "../ui";
import { Frame } from "./Frame";
import { OnboardingPreview } from "./OnboardingPreview";
import type { StepNav } from "./summary";

/**
 * Aside's sign-in window without the sign-in: openside has no accounts, so the email and Google
 * buttons become the two ways forward, starting the guide or leaving it.
 */
export function WelcomeStep({ next, leave, leaving }: StepNav) {
  return (
    <Frame step="welcome" layout="split" title={t("onboarding.welcome.title")} art={<OnboardingPreview />}>
      <div className="onb-welcome-actions">
        <Button variant="primary" disabled={leaving} onClick={next}>
          {t("onboarding.page.getStarted")}
        </Button>
        <Button disabled={leaving} onClick={() => void leave("skipped")}>
          {t("onboarding.welcome.skip")}
        </Button>
      </div>
    </Frame>
  );
}
