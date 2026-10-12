import { useEffect, useRef } from "react";
import { mayUse } from "../auth";
import { useClient } from "../context";
import type { AuthButton } from "../otp/authCard";
import {
  isApprovalTicket,
  isChoiceTicket,
  isLoginTicket,
  mountApprovalCard,
  mountChoiceForm,
  mountLoginForm,
  type ApprovalTicket,
  type ChoiceTicket,
  type LoginTicket,
} from "../otp/loginForm";
import { mountOtpForm, type OtpFormOptions, type OtpTicket } from "../otp/otpForm";
import "../otp/auth.css";

const BUTTON: Record<AuthButton, string> = {
  primary: "ui-button ui-button--primary",
  secondary: "ui-button ui-button--secondary",
  ghost: "ui-button ui-button--ghost",
};

/**
 * The shared code, login, choice and phone-approval cards (`otp/otpForm.ts`, `otp/loginForm.ts`)
 * with the web app's buttons.
 */
export function AuthForm({
  ticket,
  layout = "card",
  focus = "polite",
  onSubmitted,
}: {
  ticket: OtpTicket | LoginTicket | ChoiceTicket | ApprovalTicket;
  layout?: OtpFormOptions["layout"];
  focus?: OtpFormOptions["focus"];
  onSubmitted?: () => void;
}) {
  const client = useClient();
  const host = useRef<HTMLDivElement>(null);
  // The form is rebuilt only for another ticket: the dock re-polls tickets as new objects, and a
  // new object or callback for the same ticket must not drop half-typed input.
  const latest = useRef({ ticket, onSubmitted });
  useEffect(() => {
    latest.current = { ticket, onSubmitted };
  });

  useEffect(() => {
    if (!host.current) return;
    const current = latest.current.ticket;
    const common = {
      client,
      layout,
      focus,
      canSave: mayUse("approve"),
      buttonClass: (variant: AuthButton) => BUTTON[variant],
      onSubmitted: () => latest.current.onSubmitted?.(),
    };
    if (isLoginTicket(current)) return mountLoginForm(host.current, { ...common, ticket: current });
    if (isChoiceTicket(current)) return mountChoiceForm(host.current, { ...common, ticket: current });
    if (isApprovalTicket(current)) return mountApprovalCard(host.current, { ...common, ticket: current });
    return mountOtpForm(host.current, { ...common, ticket: current });
  }, [client, ticket.id, layout, focus]);

  return <div ref={host} className="otp-host" />;
}
