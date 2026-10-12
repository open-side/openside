import type { ButtonHTMLAttributes, ReactNode, Ref } from "react";

export const ICON_STROKE = 1.8;

export type ButtonVariant = "secondary" | "primary" | "danger" | "ghost";

export interface ButtonProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  variant?: ButtonVariant;
  size?: "default" | "compact";
  ref?: Ref<HTMLButtonElement>;
}

function cx(...names: (string | false | null | undefined)[]): string {
  return names.filter(Boolean).join(" ");
}

export function Button({ variant = "secondary", size = "default", className, type = "button", ...props }: ButtonProps) {
  return (
    <button
      {...props}
      type={type}
      className={cx("ui-button", `ui-button--${variant}`, size === "compact" && "ui-button--compact", className)}
    />
  );
}

export interface IconButtonProps extends Omit<ButtonHTMLAttributes<HTMLButtonElement>, "aria-label"> {
  /** Accessible name; icon-only controls have no visible text. */
  label: string;
  outlined?: boolean;
  /** `primary` is the accent-filled circle used by send / stop. */
  tone?: "default" | "primary";
  size?: "default" | "small";
  ref?: Ref<HTMLButtonElement>;
  children: ReactNode;
}

export function IconButton({ label, outlined, tone = "default", size = "default", className, type = "button", title, ...props }: IconButtonProps) {
  return (
    <button
      {...props}
      type={type}
      aria-label={label}
      title={title ?? label}
      className={cx(
        "ui-icon-button",
        outlined && "ui-icon-button--outlined",
        tone === "primary" && "ui-icon-button--primary",
        size === "small" && "ui-icon-button--small",
        className,
      )}
    />
  );
}

export { cx };
