import type { LucideIcon } from "lucide-react";
import { ChevronRight, Ellipsis, LockKeyhole, Plus, TriangleAlert } from "lucide-react";
import { useCallback, useState, type HTMLAttributes, type ReactNode } from "react";
import type { Resource } from "../hooks/useResource";
import { Button, cx, Dialog, DialogActions, ICON_STROKE, IconButton, PopoverMenu, Spinner } from "../ui";
import "./pages.css";

/**
 * A block of a Workspace section (aside-relay `SectionList`). Without a title, `summary` (a count
 * or status line) sits left of the actions so the header row is never empty space.
 */
export function SectionList({
  title,
  summary,
  description,
  actions,
  children,
  label,
}: {
  title?: string;
  summary?: ReactNode;
  description?: ReactNode;
  actions?: ReactNode;
  children?: ReactNode;
  /** Accessible name of the region when the title alone is ambiguous. */
  label?: string;
}) {
  return (
    <section className="section-list" aria-label={label ?? title}>
      {(title || summary || actions) && (
        <div className="section-list-header">
          {title ? <h3 className="section-list-title">{title}</h3> : <p className="section-list-summary">{summary}</p>}
          {actions && <div className="section-list-actions">{actions}</div>}
        </div>
      )}
      {description && <p className="section-list-description">{description}</p>}
      {children}
    </section>
  );
}

/** The "+ New …" button in a section header; a second add button next to it is secondary. */
export function AddButton({
  onClick,
  variant = "primary",
  children,
}: {
  onClick: () => void;
  variant?: "primary" | "secondary";
  children: ReactNode;
}) {
  return (
    <Button variant={variant} size="compact" onClick={onClick}>
      <Plus size={16} strokeWidth={ICON_STROKE} aria-hidden="true" />
      {children}
    </Button>
  );
}

export type BadgeTone = "neutral" | "success" | "attention" | "danger";

export function Badge({ tone = "neutral", children }: { tone?: BadgeTone; children: ReactNode }) {
  return <span className={cx("badge", `badge--${tone}`)}>{children}</span>;
}

export interface ResourceCardProps extends Omit<HTMLAttributes<HTMLElement>, "title"> {
  title: ReactNode;
  detail?: ReactNode;
  badge?: ReactNode;
  /** Colour carries state only: success (ready), attention (paused, limited), danger (rejected). */
  badgeTone?: BadgeTone;
  /** Leading glyph in a neutral tile. */
  icon?: LucideIcon;
  /** Controls at the end of the header row: a switch, a select or the row menu. */
  trailing?: ReactNode;
  children?: ReactNode;
}

/** One row of a `resource-list` (a grouped list), or a standalone card outside one. */
export function ResourceCard({ title, detail, badge, badgeTone, icon: Icon, trailing, children, className, ...rest }: ResourceCardProps) {
  return (
    <article {...rest} className={cx("resource-card", className)}>
      <div className="resource-card-header">
        {Icon && (
          <span className="resource-card-icon" aria-hidden="true">
            <Icon size={18} strokeWidth={ICON_STROKE} />
          </span>
        )}
        <div className="resource-card-copy">
          <h4 className="resource-card-title">
            <span>{title}</span>
            {badge && <Badge tone={badgeTone}>{badge}</Badge>}
          </h4>
          {detail && <p className="resource-card-detail">{detail}</p>}
        </div>
        {trailing && <div className="resource-card-trailing">{trailing}</div>}
      </div>
      {children}
    </article>
  );
}

/** The "…" menu of a row; items are `MenuItem`s. Named like the conversation rows' menus. */
export function RowMenu({ name, children }: { name: string; children: ReactNode }) {
  return (
    <PopoverMenu
      label={`Options for ${name}`}
      trigger={
        <IconButton label={`Options for ${name}`} className="row-menu-trigger">
          <Ellipsis size={18} strokeWidth={ICON_STROKE} aria-hidden="true" />
        </IconButton>
      }
    >
      {children}
    </PopoverMenu>
  );
}

/** Shows or hides detail inside a row (run history, change history). */
export function Disclosure({ expanded, onToggle, children }: { expanded: boolean; onToggle: () => void; children: ReactNode }) {
  return (
    <button type="button" className="row-disclosure" aria-expanded={expanded} onClick={onToggle}>
      <ChevronRight size={16} strokeWidth={ICON_STROKE} aria-hidden="true" />
      {children}
    </button>
  );
}

export function CardActions({ children }: { children: ReactNode }) {
  return <div className="card-actions">{children}</div>;
}

export function StatusRow({ label, children }: { label: ReactNode; children: ReactNode }) {
  return (
    <div className="status-row">
      <span>{label}</span>
      <span className="status-row-value">{children}</span>
    </div>
  );
}

export function EmptyState({ icon: Icon, title, detail, action }: { icon: LucideIcon; title: string; detail?: ReactNode; action?: ReactNode }) {
  return (
    <div className="empty-state">
      <span className="empty-state-icon" aria-hidden="true">
        <Icon size={22} strokeWidth={ICON_STROKE} />
      </span>
      <strong>{title}</strong>
      {detail && <p>{detail}</p>}
      {action}
    </div>
  );
}

export function Loading({ label = "Loading…" }: { label?: string }) {
  return (
    <p className="loading-row" role="status">
      <Spinner size={16} />
      <span>{label}</span>
    </p>
  );
}

export function ErrorNote({ children }: { children: ReactNode }) {
  return (
    <p role="alert" className="error-note">
      {children}
    </p>
  );
}

/**
 * The states every Workspace list shares: loading, a 403 (scope), a failed load with a retry,
 * then the data, with a later action error shown above it.
 */
export function ResourceView<T>({
  resource,
  what,
  children,
}: {
  resource: Resource<T>;
  /** What the screen manages, for the scope notice ("skills", "the vault"). */
  what: string;
  children: (data: T) => ReactNode;
}) {
  if (resource.forbidden) return <ScopeNotice what={what} />;
  if (!resource.data) {
    return resource.error ? (
      <div className="load-error" role="alert">
        <TriangleAlert size={18} strokeWidth={ICON_STROKE} aria-hidden="true" />
        <p>{resource.error}</p>
        <Button size="compact" onClick={() => void resource.reload()}>
          Try again
        </Button>
      </div>
    ) : (
      <Loading />
    );
  }
  return (
    <>
      {resource.error ? <ErrorNote>{resource.error}</ErrorNote> : null}
      {children(resource.data)}
    </>
  );
}

/** A 403: this device was paired without the `admin` scope every Workspace section needs. */
export function ScopeNotice({ what }: { what: string }) {
  return (
    <div className="empty-state scope-notice" role="alert">
      <span className="empty-state-icon" aria-hidden="true">
        <LockKeyhole size={22} strokeWidth={ICON_STROKE} />
      </span>
      <strong>This device needs the admin scope</strong>
      <p>
        It was paired without permission to manage {what}. On the computer running openside, run{" "}
        <code>openside pair --scope admin</code> and open the new link on this device.
      </p>
    </div>
  );
}

/** Explicit confirmation before a destructive action (replaces `window.confirm`). */
export function ConfirmDialog({
  open,
  title,
  description,
  confirmLabel,
  onConfirm,
  onClose,
}: {
  open: boolean;
  title: string;
  description: ReactNode;
  confirmLabel: string;
  onConfirm: () => void;
  onClose: () => void;
}) {
  return (
    <Dialog open={open} onClose={onClose} title={title} description={description}>
      <DialogActions>
        <Button onClick={onClose}>Cancel</Button>
        <Button
          variant="primary"
          className="confirm-danger"
          onClick={() => {
            onClose();
            onConfirm();
          }}
        >
          {confirmLabel}
        </Button>
      </DialogActions>
    </Dialog>
  );
}

/**
 * Open state plus the value a dialog shows. The value outlives `open` so the dialog keeps its
 * content while it animates out.
 */
export function useDialogState<T>() {
  const [value, setValue] = useState<T | null>(null);
  const [open, setOpen] = useState(false);
  const show = useCallback((next: T) => {
    setValue(next);
    setOpen(true);
  }, []);
  const close = useCallback(() => setOpen(false), []);
  return { value, open, show, close };
}

export interface ConfirmRequest {
  title: string;
  description: ReactNode;
  confirmLabel: string;
  onConfirm: () => void;
}

/** One confirmation dialog per screen; returns the element to render and `ask`. */
export function useConfirm(): [ReactNode, (request: ConfirmRequest) => void] {
  const { value, open, show, close } = useDialogState<ConfirmRequest>();
  const element = value && (
    <ConfirmDialog
      open={open}
      title={value.title}
      description={value.description}
      confirmLabel={value.confirmLabel}
      onConfirm={value.onConfirm}
      onClose={close}
    />
  );
  return [element, show];
}
