import type { LucideIcon } from "lucide-react";
import { Plus } from "lucide-react";
import { useCallback, useState, type HTMLAttributes, type ReactNode } from "react";
import { Button, cx, Dialog, DialogActions, ICON_STROKE, Spinner } from "../ui";
import "./pages.css";

/** A titled block of a Workspace section, with its actions on the right (aside-relay `SectionList`). */
export function SectionList({
  title,
  description,
  actions,
  children,
  label,
}: {
  title?: string;
  description?: ReactNode;
  actions?: ReactNode;
  children?: ReactNode;
  /** Accessible name of the region when the title alone is ambiguous. */
  label?: string;
}) {
  return (
    <section className="section-list" aria-label={label ?? title}>
      {(title || actions) && (
        <div className={cx("section-list-header", !title && "section-list-header--actions")}>
          {title && <h3 className="section-list-title">{title}</h3>}
          {actions && <div className="section-list-actions">{actions}</div>}
        </div>
      )}
      {description && <p className="section-list-description">{description}</p>}
      {children}
    </section>
  );
}

/** The primary "+ New …" button in a section header. */
export function AddButton({ onClick, children }: { onClick: () => void; children: ReactNode }) {
  return (
    <Button variant="primary" size="compact" onClick={onClick}>
      <Plus size={16} strokeWidth={ICON_STROKE} aria-hidden="true" />
      {children}
    </Button>
  );
}

export interface ResourceCardProps extends Omit<HTMLAttributes<HTMLElement>, "title"> {
  title: ReactNode;
  detail?: ReactNode;
  badge?: ReactNode;
  /** `attention` tints the badge for paused, blocked or failed states. */
  badgeTone?: "accent" | "muted" | "attention";
  children?: ReactNode;
}

export function ResourceCard({ title, detail, badge, badgeTone = "accent", children, className, ...rest }: ResourceCardProps) {
  return (
    <article {...rest} className={cx("resource-card", className)}>
      <div className="resource-card-header">
        <div className="resource-card-copy">
          <h4 className="resource-card-title">{title}</h4>
          {detail && <p className="resource-card-detail">{detail}</p>}
        </div>
        {badge && <span className={cx("badge", `badge--${badgeTone}`)}>{badge}</span>}
      </div>
      {children}
    </article>
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
      <Icon size={26} strokeWidth={ICON_STROKE} aria-hidden="true" />
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
