import { ArrowUpRight, ChevronRight, Info } from "lucide-react";
import { useId, type ReactNode } from "react";
import { Link } from "react-router";
import { cx, ICON_STROKE } from "./Button";

/** The ids a row's control uses to take its name and description from the row's text. */
export interface SettingsRowIds {
  labelId: string;
  descriptionId: string | undefined;
}

interface RowCommon {
  title: ReactNode;
  description?: ReactNode;
  /** A 16px icon or 32px avatar tile before the text (Security, Account). */
  leading?: ReactNode;
  /** A badge right after the title ("New", "This device"). */
  badge?: ReactNode;
}

export type SettingsRowProps = RowCommon &
  (
    | {
        /**
         * The control at the row's end. A function receives the title/description ids, so a
         * `Switch` or `Select` is named by the row (`aria-labelledby`) instead of a second label.
         */
        control?: ReactNode | ((ids: SettingsRowIds) => ReactNode);
        to?: never;
        href?: never;
        onClick?: never;
      }
    | { to: string; state?: unknown; control?: never; href?: never; onClick?: never }
    | { href: string; control?: never; to?: never; onClick?: never }
    | { onClick: () => void; external?: boolean; control?: never; to?: never; href?: never }
  );

/**
 * One settings row (Aside settings §2): 56px with a description, 52px without; title 14/500,
 * description 12 muted, 14px start and 10px end padding. `to` makes the whole row an in-app
 * link with ›; `href` an external link with ↗; `onClick` a button (↗ when `external`).
 */
export function SettingsRow(props: SettingsRowProps) {
  const labelId = useId();
  const descriptionId = useId();
  const { title, description, leading, badge } = props;
  const ids = { labelId, descriptionId: description ? descriptionId : undefined };
  const copy = (
    <>
      {leading && (
        <span className="ui-row-leading" aria-hidden="true">
          {leading}
        </span>
      )}
      <span className="ui-row-copy">
        <span className="ui-row-title">
          <span id={labelId}>{title}</span>
          {badge}
        </span>
        {description && (
          <span className="ui-row-description" id={descriptionId}>
            {description}
          </span>
        )}
      </span>
    </>
  );
  const className = cx("ui-row", !description && "ui-row--single");
  const glyph = (external: boolean) =>
    external ? <ArrowUpRight className="ui-row-glyph" strokeWidth={ICON_STROKE} aria-hidden="true" /> : <ChevronRight className="ui-row-glyph" strokeWidth={ICON_STROKE} aria-hidden="true" />;

  if (props.to !== undefined) {
    return (
      <Link className={cx(className, "ui-row--action")} to={props.to} state={props.state}>
        {copy}
        {glyph(false)}
      </Link>
    );
  }
  if (props.href !== undefined) {
    return (
      <a className={cx(className, "ui-row--action")} href={props.href} target="_blank" rel="noopener noreferrer">
        {copy}
        {glyph(true)}
      </a>
    );
  }
  if (props.onClick !== undefined) {
    return (
      <button type="button" className={cx(className, "ui-row--action")} onClick={props.onClick}>
        {copy}
        {glyph(props.external === true)}
      </button>
    );
  }
  const control = typeof props.control === "function" ? props.control(ids) : props.control;
  return (
    <div className={className} role="group" aria-labelledby={labelId} aria-describedby={ids.descriptionId}>
      {copy}
      {control && <div className="ui-row-control">{control}</div>}
    </div>
  );
}

/** A rounded card of rows split by full-bleed hairlines. */
export function SettingsCard({ children, className }: { children: ReactNode; className?: string }) {
  return <div className={cx("ui-card", className)}>{children}</div>;
}

/**
 * A labelled run of cards ("Preferences", "Tabs"): the 14px muted label, then the cards 8px
 * apart; consecutive sections sit 32px apart. `info` adds Aside's ⓘ after the label, its text
 * shown on hover and read as the icon's name.
 */
export function SettingsSection({ label, info, children }: { label?: string; info?: string; children: ReactNode }) {
  const id = useId();
  return (
    <section className="ui-section" aria-labelledby={label ? id : undefined}>
      {label && (
        <h2 className="ui-section-label" id={id}>
          {label}
          {info && (
            <span className="ui-section-info" role="img" aria-label={info} title={info}>
              <Info strokeWidth={ICON_STROKE} aria-hidden="true" />
            </span>
          )}
        </h2>
      )}
      {children}
    </section>
  );
}
