import { ChevronRight, Download, KeyRound } from "lucide-react";
import { useCallback, useState } from "react";
import { t } from "../i18n";
import { ICON_STROKE } from "../ui";
import { Frame, PrimaryButton } from "./Frame";
import type { StepNav } from "./summary";
import { VaultImportSheet } from "./VaultImportSheet";
import "./import.css";

/** Decoration for the split's right side: a sign-in form with openside's fill suggestion under the password field. */
function AutofillArt() {
  return (
    <div className="onb-pw-art">
      <div className="onb-pw-form">
        <span className="onb-pw-field">
          <span className="onb-pw-line" />
        </span>
        <span className="onb-pw-field" data-focused="">
          <span className="onb-pw-dots">••••••••••</span>
        </span>
        <span className="onb-pw-chip">
          <KeyRound size={14} strokeWidth={ICON_STROKE} />
          {t("onboarding.passwords.artChip")}
        </span>
        <span className="onb-pw-submit" />
      </div>
    </div>
  );
}

/**
 * The guide's passwords step. Aside's second row ("Connect my password manager") is left out:
 * openside cannot connect to another manager, only import what it exports.
 */
export function PasswordsStep({ next, back, summary, update }: StepNav) {
  const [open, setOpen] = useState(false);
  const [note, setNote] = useState<string | null>(null);
  // Frame's window element (`.onb-window`) hosts the sheet, so the dim and blur stay inside it.
  const [host, setHost] = useState<HTMLElement | null>(null);
  const findHost = useCallback((list: HTMLElement | null) => setHost(list?.closest<HTMLElement>(".onb-window") ?? null), []);
  const count = summary.passwords;
  const imported = count !== null;
  return (
    <Frame
      step="passwords"
      layout="split"
      title={t("onboarding.passwords.title")}
      lead={t("onboarding.passwords.lead")}
      art={<AutofillArt />}
      onBack={back}
      skip={imported ? null : { label: t("onboarding.frame.skip"), onClick: next }}
      primary={imported ? <PrimaryButton onClick={next}>{t("common.continue")}</PrimaryButton> : <PrimaryButton onClick={() => setOpen(true)}>{t("onboarding.passwords.import")}</PrimaryButton>}
    >
      <ul className="onb-pw-options" ref={findHost}>
        <li>
          <button type="button" className="onb-pw-option" onClick={() => setOpen(true)}>
            <span className="onb-pw-option-icon" aria-hidden="true">
              <Download size={16} strokeWidth={ICON_STROKE} />
            </span>
            <span className="onb-pw-option-copy">
              <strong>{t("onboarding.passwords.importRow")}</strong>
              <span>{t("onboarding.passwords.importRowDetail")}</span>
            </span>
            <ChevronRight className="onb-pw-option-chevron" size={16} strokeWidth={ICON_STROKE} aria-hidden="true" />
          </button>
        </li>
      </ul>
      {imported ? (
        <p className="onb-note" role="status">
          {count === 0 ? t("onboarding.passwords.none") : t("onboarding.passwords.added", { count })}
          {note ? ` ${note}` : null}
        </p>
      ) : null}
      <VaultImportSheet
        open={open}
        container={host}
        onClose={() => setOpen(false)}
        onImported={(added, detail) => {
          setOpen(false);
          setNote(detail);
          update({ passwords: (count ?? 0) + added });
        }}
      />
    </Frame>
  );
}
