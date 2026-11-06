import { ArrowUp, SquarePen } from "lucide-react";
import { t } from "../i18n";
import { ICON_STROKE } from "../ui";

/**
 * The welcome step's art: a miniature openside window on a sky gradient, drawn from the theme
 * tokens. Skeleton bars stand in for content on purpose: no names, numbers or screenshots that
 * could be mistaken for the person's own data.
 */
export function OnboardingPreview() {
  return (
    <div className="onb-preview">
      <div className="onb-mini">
        <div className="onb-mini-sidebar">
          <span className="onb-mini-lights">
            <span />
            <span />
            <span />
          </span>
          <span className="onb-mini-pins">
            <span />
            <span />
            <span />
            <span />
            <span />
            <span />
          </span>
          <span className="onb-mini-heading" />
          <span className="onb-mini-new">
            <SquarePen size={11} strokeWidth={ICON_STROKE} />
            {t("onboarding.preview.newTask")}
          </span>
          <span className="onb-mini-row" />
          <span className="onb-mini-row" />
          <span className="onb-mini-row" />
          <span className="onb-mini-heading" />
          <span className="onb-mini-row" />
          <span className="onb-mini-row" />
          <span className="onb-mini-row" />
        </div>
        <div className="onb-mini-main">
          <span className="onb-mini-toolbar">
            <span />
          </span>
          <span className="onb-mini-composer">
            <span>{t("onboarding.preview.composer")}</span>
            <span className="onb-mini-send">
              <ArrowUp size={11} strokeWidth={2.4} />
            </span>
          </span>
          <span className="onb-mini-tasks">
            <span className="onb-mini-task">
              <span />
              <span />
              <span />
              <span className="onb-mini-shot" />
            </span>
            <span className="onb-mini-task">
              <span />
              <span />
              <span />
              <span className="onb-mini-shot" />
            </span>
          </span>
        </div>
      </div>
    </div>
  );
}
