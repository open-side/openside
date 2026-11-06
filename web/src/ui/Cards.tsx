import { Info } from "lucide-react";
import type { ReactNode } from "react";
import { cx, ICON_STROKE } from "./Button";

export interface HeroCardProps {
  title: string;
  description: ReactNode;
  /** The call to action under the copy (a `Button`). */
  action?: ReactNode;
  /** The 288×165 picture at the card's end, drawn by the caller. */
  illustration?: ReactNode;
}

/** The feature card on top of a settings page (Developers, Mini popup, Lasso): copy, action, picture. */
export function HeroCard({ title, description, action, illustration }: HeroCardProps) {
  return (
    <section className="ui-hero">
      <div className="ui-hero-copy">
        <h2 className="ui-hero-title">{title}</h2>
        <p className="ui-hero-description">{description}</p>
        {action && <div className="ui-hero-action">{action}</div>}
      </div>
      {illustration && (
        <div className="ui-hero-art" aria-hidden="true">
          {illustration}
        </div>
      )}
    </section>
  );
}

export interface EmptyStateProps {
  /** A 16px glyph shown in the 32px tile. */
  icon: ReactNode;
  title: string;
  description?: string;
  action?: ReactNode;
}

/** A settings card with nothing in it yet (Archived chats): tile, title and line, centred. */
export function EmptyState({ icon, title, description, action }: EmptyStateProps) {
  return (
    <div className="ui-empty" role="status">
      <span className="ui-empty-tile" aria-hidden="true">
        {icon}
      </span>
      <p className="ui-empty-title">{title}</p>
      {description && <p className="ui-empty-description">{description}</p>}
      {action}
    </div>
  );
}

export interface InfoBannerProps {
  /**
   * `neutral` is the grey strip on top of a card (36px, one line); `info` is the sky strip with
   * a bold first line (Plugins › MCPs).
   */
  tone?: "neutral" | "info";
  title?: string;
  children: ReactNode;
  /** A small button at the strip's end. */
  action?: ReactNode;
}

export function InfoBanner({ tone = "neutral", title, children, action }: InfoBannerProps) {
  return (
    <div className={cx("ui-banner", tone === "info" && "ui-banner--info")} role="note">
      <Info className="ui-banner-icon" strokeWidth={ICON_STROKE} aria-hidden="true" />
      <div className="ui-banner-copy">
        {title && <p className="ui-banner-title">{title}</p>}
        <div className="ui-banner-text">{children}</div>
      </div>
      {action}
    </div>
  );
}
