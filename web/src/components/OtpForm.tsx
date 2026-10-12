import { useEffect, useRef } from "react";
import { mayApprove } from "../auth";
import { useClient } from "../context";
import { mountOtpForm, type OtpButton, type OtpFormOptions, type OtpTicket } from "../otp/otpForm";
import "../otp/otp.css";

const BUTTON: Record<OtpButton, string> = {
  primary: "ui-button ui-button--primary",
  secondary: "ui-button ui-button--secondary",
  ghost: "ui-button ui-button--ghost",
};

/** The shared one-time-code form (`otp/otpForm.ts`) with the web app's buttons. */
export function OtpForm({
  ticket,
  layout = "card",
  focus = "polite",
  onSubmitted,
}: {
  ticket: OtpTicket;
  layout?: OtpFormOptions["layout"];
  focus?: OtpFormOptions["focus"];
  onSubmitted?: () => void;
}) {
  const client = useClient();
  const host = useRef<HTMLDivElement>(null);
  // The form is rebuilt only for another ticket: the dock re-polls tickets as new objects, and a
  // new object or callback for the same ticket must not drop a half-typed code.
  const latest = useRef({ ticket, onSubmitted });
  useEffect(() => {
    latest.current = { ticket, onSubmitted };
  });

  useEffect(() => {
    if (!host.current) return;
    return mountOtpForm(host.current, {
      ticket: latest.current.ticket,
      client,
      layout,
      focus,
      canSave: mayApprove(),
      buttonClass: (variant) => BUTTON[variant],
      onSubmitted: () => latest.current.onSubmitted?.(),
    });
  }, [client, ticket.id, layout, focus]);

  return <div ref={host} className="otp-host" />;
}
