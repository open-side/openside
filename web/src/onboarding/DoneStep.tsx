import { ChevronRight, Palette, SquareTerminal } from "lucide-react";
import { useState, type ReactNode } from "react";
import { t } from "../i18n";
import { currentOffer } from "../pwa/install";
import { settingsPath } from "../settings/nav";
import { useBrowserPage } from "../settings/pages/personalShared";
import { Button, Dialog, DialogActions, ICON_STROKE } from "../ui";
import { Frame, PrimaryButton } from "./Frame";
import type { StepNav } from "./summary";

function Drill({ icon, title, detail, disabled, onClick }: { icon: ReactNode; title: string; detail: string; disabled: boolean; onClick: () => void }) {
  return (
    <li>
      <button type="button" className="onb-drill" disabled={disabled} onClick={onClick}>
        <span className="onb-drill-icon" aria-hidden="true">
          {icon}
        </span>
        <span className="onb-row-copy">
          <span className="onb-row-title">{title}</span>
          <span className="onb-row-detail">{detail}</span>
        </span>
        <ChevronRight className="onb-drill-chevron" size={16} strokeWidth={ICON_STROKE} aria-hidden="true" />
      </button>
    </li>
  );
}

function Option({ title, detail, checked, onChange }: { title: string; detail: string; checked: boolean; onChange: (checked: boolean) => void }) {
  return (
    <li>
      <label className="onb-option">
        <input type="checkbox" checked={checked} onChange={(event) => onChange(event.target.checked)} />
        <span className="onb-row-copy">
          <span className="onb-row-title">{title}</span>
          <span className="onb-row-detail">{detail}</span>
        </span>
      </label>
    </li>
  );
}

/**
 * Aside's "You're all set!": two shortcuts into Settings and the finishing options, ticked by default.
 * No engine can make openside the OS default browser, so that option opens the browser's own
 * setting; the launcher option exists only while the browser offers to install the app. Aside's
 * "Help improve" option is left out because openside collects nothing.
 */
export function DoneStep({ leave, leaving }: StepNav) {
  const openPage = useBrowserPage();
  // Read once: the browser's install prompt can be used a single time, so the offer must not change under the person.
  const [offer] = useState(() => {
    const current = currentOffer();
    return current?.kind === "android" ? current : null;
  });
  const [defaultBrowser, setDefaultBrowser] = useState(true);
  const [launcher, setLauncher] = useState(true);
  const [asking, setAsking] = useState(false);

  function start() {
    // The install prompt needs the click's user activation, so it goes first and is not awaited.
    if (offer && launcher) void offer.install();
    if (defaultBrowser) setAsking(true);
    else void leave("completed");
  }

  return (
    <Frame
      step="done"
      layout="centered"
      title={t("onboarding.done.title")}
      lead={t("onboarding.done.lead")}
      primary={
        <PrimaryButton disabled={leaving} onClick={start}>
          {t("onboarding.page.getStarted")}
        </PrimaryButton>
      }
    >
      <div className="onb-done">
        <ul className="onb-list" aria-label={t("onboarding.done.moreLabel")}>
          <Drill
            icon={<Palette size={16} strokeWidth={ICON_STROKE} />}
            title={t("onboarding.done.appearance")}
            detail={t("onboarding.done.appearanceDetail")}
            disabled={leaving}
            onClick={() => void leave("completed", settingsPath("appearance"))}
          />
          <Drill
            icon={<SquareTerminal size={16} strokeWidth={ICON_STROKE} />}
            title={t("onboarding.done.cli")}
            detail={t("onboarding.done.cliDetail")}
            disabled={leaving}
            onClick={() => void leave("completed", settingsPath("developers"))}
          />
        </ul>
        <ul className="onb-list" aria-label={t("onboarding.done.optionsLabel")}>
          <Option title={t("onboarding.done.defaultBrowser")} detail={t("onboarding.done.defaultBrowserDetail")} checked={defaultBrowser} onChange={setDefaultBrowser} />
          {offer ? <Option title={t("onboarding.done.launcher")} detail={t("onboarding.done.launcherDetail")} checked={launcher} onChange={setLauncher} /> : null}
        </ul>
      </div>
      <Dialog open={asking} onClose={() => setAsking(false)} title={t("onboarding.done.defaultTitle")} description={t("onboarding.done.defaultBody")} className="onb-default-dialog">
        <DialogActions>
          <Button disabled={leaving} onClick={() => void leave("completed")}>
            {t("onboarding.done.notNow")}
          </Button>
          <Button
            variant="primary"
            disabled={leaving}
            onClick={() => {
              openPage("chrome://settings");
              void leave("completed");
            }}
          >
            {t("onboarding.done.openSettings")}
          </Button>
        </DialogActions>
      </Dialog>
    </Frame>
  );
}
