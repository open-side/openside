import { useId } from "react";
import { t } from "../../i18n";
import { ErrorNote } from "../../pages/blocks";
import { RestartNotice } from "../../pages/RestartNotice";
import { HeroCard, SettingsCard, SettingsRow, SettingsSection, Switch } from "../../ui";
import { useMemoryConfig } from "./memoryConfig";
import "./memory.css";

/** openside's own picture: page cards drifting through a sky towards a memory cloud. */
function Sky() {
  const id = useId();
  const sky = `${id}-sky`;
  const glow = `${id}-glow`;
  return (
    <svg viewBox="0 0 288 165" role="presentation" preserveAspectRatio="xMidYMid slice">
      <defs>
        <linearGradient id={sky} x1="0" y1="0" x2="1" y2="1">
          <stop offset="0" stopColor="#38bdf8" />
          <stop offset="0.55" stopColor="#7dd3fc" />
          <stop offset="1" stopColor="#c4b5fd" />
        </linearGradient>
        <radialGradient id={glow} cx="0.5" cy="0.5" r="0.5">
          <stop offset="0" stopColor="#ffffff" stopOpacity="0.95" />
          <stop offset="1" stopColor="#ffffff" stopOpacity="0" />
        </radialGradient>
      </defs>
      <rect width="288" height="165" fill={`url(#${sky})`} />
      <ellipse cx="150" cy="104" rx="92" ry="40" fill={`url(#${glow})`} />
      <g fill="#ffffff" opacity="0.92">
        <circle cx="122" cy="104" r="22" />
        <circle cx="150" cy="94" r="28" />
        <circle cx="180" cy="106" r="20" />
        <rect x="100" y="104" width="100" height="22" rx="11" />
      </g>
      <g stroke="#ffffff" strokeOpacity="0.7" strokeDasharray="2 4" fill="none">
        <path d="M58 46 C 90 60, 110 70, 132 86" />
        <path d="M232 38 C 210 56, 190 70, 170 84" />
        <path d="M250 120 C 230 118, 214 114, 196 108" />
      </g>
      <g>
        <rect x="30" y="24" width="44" height="34" rx="8" fill="#ffffff" />
        <rect x="37" y="32" width="22" height="4" rx="2" fill="#0ea5e9" />
        <rect x="37" y="40" width="30" height="3" rx="1.5" fill="#cbd5e1" />
        <rect x="37" y="46" width="24" height="3" rx="1.5" fill="#cbd5e1" />
      </g>
      <g>
        <rect x="214" y="16" width="44" height="34" rx="8" fill="#ffffff" />
        <circle cx="226" cy="28" r="5" fill="#f59e0b" />
        <rect x="235" y="25" width="16" height="3" rx="1.5" fill="#cbd5e1" />
        <rect x="221" y="38" width="30" height="3" rx="1.5" fill="#cbd5e1" />
      </g>
      <g>
        <rect x="236" y="104" width="38" height="30" rx="8" fill="#ffffff" />
        <rect x="243" y="111" width="24" height="10" rx="3" fill="#a78bfa" />
        <rect x="243" y="125" width="16" height="3" rx="1.5" fill="#cbd5e1" />
      </g>
      <g>
        <rect x="16" y="112" width="36" height="28" rx="8" fill="#ffffff" />
        <path d="M26 132 l6 -8 l5 5 l4 -6 l6 9 z" fill="#34d399" />
      </g>
      <path d="M146 60 l0 16 l4 -4 l3 7 l3 -1 l-3 -7 l6 0 z" fill="#0f172a" stroke="#ffffff" strokeWidth="1" />
    </svg>
  );
}

/**
 * Settings › Memory › Context Awareness. openside has no device-wide activity capture; the
 * nearest engine is `[memory] history`, which records pages visited in openside's own browser,
 * so the switch drives that key and a note says what it does not cover.
 */
export function ContextAwareness() {
  const config = useMemoryConfig();
  const enabled = config.memory?.history ?? false;
  return (
    <div className="memory-page" data-section="context-awareness">
      <HeroCard title={t("memory.context.heroTitle")} description={t("memory.context.heroDescription")} illustration={<Sky />} />
      <SettingsSection label={t("memory.context.capture")}>
        <SettingsCard>
          <SettingsRow
            title={t("memory.context.enable.title")}
            description={t("memory.context.enable.description")}
            control={(ids) => (
              <Switch
                aria-labelledby={ids.labelId}
                aria-describedby={ids.descriptionId}
                checked={enabled}
                disabled={!config.memory || config.busy}
                onChange={(next) => void config.set("history", next)}
              />
            )}
          />
        </SettingsCard>
        <p className="memory-note">{t("memory.context.note")}</p>
        {config.error ? <ErrorNote>{config.error}</ErrorNote> : null}
        {config.restartRequired ? <RestartNotice onRestarted={() => void config.reload()}>{t("memory.restart")}</RestartNotice> : null}
      </SettingsSection>
    </div>
  );
}
