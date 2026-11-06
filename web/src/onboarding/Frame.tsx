import { ChevronLeft, CornerDownLeft } from "lucide-react";
import { useEffect, useRef, type ReactNode } from "react";
import { t } from "../i18n";
import { BrandMark, Button, cx, ICON_STROKE, type ButtonProps } from "../ui";
import type { StepId } from "./summary";

/** Steps that show the pager dots; welcome and done stand outside the count, as in Aside. */
const PAGED: readonly StepId[] = ["import", "passwords", "subscription"];

export interface FrameProps {
  step: StepId;
  layout: "centered" | "split";
  title: string;
  lead?: string;
  /** Right panel of the split layout; decorative, so it is hidden from assistive tech. */
  art?: ReactNode;
  onBack?: () => void;
  skip?: { label: string; onClick: () => void } | null;
  primary?: ReactNode;
  /** Ref callback of the footer's primary container, for a step that portals its own button there. */
  primarySlot?: (el: HTMLDivElement | null) => void;
  children: ReactNode;
}

/** The guide's black action button: the label and a return glyph, like Aside's "Connect ↵". */
export function PrimaryButton({ children, className, ...props }: ButtonProps) {
  return (
    <Button variant="primary" {...props} className={cx("onb-primary-button", className)}>
      {children}
      <CornerDownLeft className="onb-enter" size={12} strokeWidth={ICON_STROKE} aria-hidden="true" />
    </Button>
  );
}

/**
 * One step of the guide drawn as Aside's onboarding window: `centered` for list steps (title, lead and
 * body down the middle, footer across the window) and `split` for the welcome and passwords steps
 * (copy and footer on the left, art on the right).
 */
export function Frame({ step, layout, title, lead, art, onBack, skip, primary, primarySlot, children }: FrameProps) {
  const heading = useRef<HTMLHeadingElement>(null);
  const position = PAGED.indexOf(step);

  // Each step mounts its own frame; moving focus to the new title tells screen readers the step changed.
  useEffect(() => {
    heading.current?.focus({ preventScroll: true });
  }, [step]);

  const hasFooter = Boolean(onBack || position >= 0 || skip || primary || primarySlot);

  return (
    <div className="onb-window" data-layout={layout} data-step={step}>
      <div className="onb-pane">
        <div className="onb-body thin-scroll" key={step}>
          {step === "welcome" ? (
            <span className="onb-brand">
              <BrandMark size={22} />
              openside
            </span>
          ) : step === "done" ? (
            <span className="onb-logo">
              <BrandMark size={30} />
            </span>
          ) : null}
          <header className="onb-head">
            <h1 id="onb-title" tabIndex={-1} ref={heading}>
              {title}
            </h1>
            {lead ? <p className="onb-lead">{lead}</p> : null}
          </header>
          {children}
        </div>
        {hasFooter ? (
          <footer className={cx("onb-footer", !onBack && position < 0 && "onb-footer--center")}>
            <div className="onb-footer-start">
              {onBack ? (
                <Button className="onb-back" onClick={onBack}>
                  <ChevronLeft size={14} strokeWidth={ICON_STROKE} aria-hidden="true" />
                  {t("common.back")}
                </Button>
              ) : null}
            </div>
            {position >= 0 ? (
              <ol className="onb-pager" aria-label={t("onboarding.frame.pager", { current: position + 1, total: PAGED.length })}>
                {PAGED.map((id, index) => (
                  <li key={id} data-state={index < position ? "past" : index === position ? "current" : "future"} aria-current={index === position ? "step" : undefined} />
                ))}
              </ol>
            ) : null}
            <div className="onb-footer-end">
              {skip ? (
                <Button variant="ghost" className="onb-skip" onClick={skip.onClick}>
                  {skip.label}
                </Button>
              ) : null}
              {primary || primarySlot ? (
                <div className="onb-primary" ref={primarySlot}>
                  {primary}
                </div>
              ) : null}
            </div>
          </footer>
        ) : null}
      </div>
      {layout === "split" && art ? (
        <div className="onb-art" aria-hidden="true">
          {art}
        </div>
      ) : null}
    </div>
  );
}
