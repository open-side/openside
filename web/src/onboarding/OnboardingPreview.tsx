import { ArrowUp, SquarePen } from "lucide-react";
import { BrandMark, ICON_STROKE } from "../ui";

/**
 * The guide's right pane: a decorative miniature of the app the person is setting up, drawn
 * from the same tokens so it follows the theme. Skeleton bars stand in for content on purpose:
 * no names, numbers or screenshots that could be mistaken for the person's own data.
 */
export function OnboardingPreview() {
  return (
    <div className="onb-preview" aria-hidden="true">
      <div className="onb-mini">
        <div className="onb-mini-sidebar">
          <span className="onb-mini-brand">
            <BrandMark size={14} />
            openside
          </span>
          <span className="onb-mini-new">
            <SquarePen size={13} strokeWidth={ICON_STROKE} />
            New task
          </span>
          <span className="onb-mini-heading" />
          <span className="onb-mini-row" />
          <span className="onb-mini-row" />
          <span className="onb-mini-row" />
        </div>
        <div className="onb-mini-main">
          <span className="onb-mini-title" />
          <span className="onb-mini-composer">
            <span>Ask AI a task, @ for context</span>
            <span className="onb-mini-send">
              <ArrowUp size={12} strokeWidth={2.4} />
            </span>
          </span>
          <span className="onb-mini-tasks">
            <span className="onb-mini-task">
              <span />
              <span />
              <span />
            </span>
            <span className="onb-mini-task">
              <span />
              <span />
              <span />
            </span>
          </span>
        </div>
      </div>
    </div>
  );
}
