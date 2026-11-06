import { useId } from "react";
import { t } from "../../i18n";
import { keyLabel } from "../../shortcuts";
import { HeroCard, SettingsCard, SettingsRow, SettingsSection, Switch } from "../../ui";
import { useBrowserPage, useBrowserSettings } from "./personalShared";
import "./personal.css";

/** The global key the browser binds to the extension's `mini-popup` command when this is on. */
export const MINI_POPUP_KEYS = ["Alt", "Space"] as const;

/** openside's own picture: a small message pill floating over a warm glow. */
function FloatingPill() {
  const id = useId();
  const glow = `${id}-glow`;
  const sun = `${id}-sun`;
  return (
    <svg viewBox="0 0 288 165" role="presentation" preserveAspectRatio="xMidYMid slice">
      <defs>
        <linearGradient id={glow} x1="0" y1="0" x2="1" y2="1">
          <stop offset="0" stopColor="#fb7185" />
          <stop offset="0.5" stopColor="#f97316" />
          <stop offset="1" stopColor="#fbbf24" />
        </linearGradient>
        <radialGradient id={sun} cx="0.35" cy="0.3" r="0.6">
          <stop offset="0" stopColor="#ffffff" stopOpacity="0.55" />
          <stop offset="1" stopColor="#ffffff" stopOpacity="0" />
        </radialGradient>
      </defs>
      <rect width="288" height="165" fill={`url(#${glow})`} />
      <rect width="288" height="165" fill={`url(#${sun})`} />
      <g>
        <rect x="30" y="58" width="228" height="50" rx="12" fill="#1c1917" fillOpacity="0.88" />
        <rect x="42" y="68" width="52" height="4" rx="2" fill="#ffffff" fillOpacity="0.45" />
        <g fill="#ffffff" fillOpacity="0.7">
          <rect x="42" y="90" width="7" height="7" rx="2" />
          <circle cx="60" cy="93.5" r="3.5" />
          <rect x="70" y="90" width="7" height="7" rx="3.5" />
        </g>
        <rect x="180" y="91" width="44" height="5" rx="2.5" fill="#ffffff" fillOpacity="0.35" />
        <circle cx="241" cy="93.5" r="8" fill="#ffffff" />
        <path d="M241 98 v-8 m-3.5 3.5 l3.5 -3.5 l3.5 3.5" stroke="#1c1917" strokeWidth="1.6" fill="none" strokeLinecap="round" strokeLinejoin="round" />
      </g>
    </svg>
  );
}

/**
 * Settings › Mini popup. The popup is the openside extension's `mini-popup` command; the switch
 * asks the person's browser to bind it to Alt+Space for every window (a global shortcut Chromium
 * never assigns on its own). A stock browser cannot be asked, so it gets a link to the page where
 * the person picks the key themselves.
 */
export function MiniPopupSettings() {
  const browser = useBrowserSettings();
  const openPage = useBrowserPage();
  const keys = keyLabel(MINI_POPUP_KEYS);
  const enabled = browser.prefs?.mini_popup;
  const description =
    browser.note ?? (browser.prefs && enabled == null ? t("settings.miniPopup.extensionOff") : t("settings.miniPopup.enable.description", { keys }));
  return (
    <div className="personal-page" data-section="mini-popup">
      <HeroCard title={t("settings.miniPopup.heroTitle")} description={t("settings.miniPopup.heroDescription", { keys })} illustration={<FloatingPill />} />
      <SettingsSection>
        <SettingsCard>
          <SettingsRow
            title={t("settings.miniPopup.enable.title")}
            description={description}
            control={(ids) => (
              <Switch
                aria-labelledby={ids.labelId}
                aria-describedby={ids.descriptionId}
                disabled={enabled == null}
                checked={enabled === true}
                onChange={(checked) => browser.update({ mini_popup: checked })}
              />
            )}
          />
        </SettingsCard>
        {browser.status === "unsupported" && (
          <SettingsCard>
            <SettingsRow
              title={t("settings.miniPopup.stock.title")}
              description={t("settings.miniPopup.stock.description")}
              external
              onClick={() => openPage("chrome://extensions/shortcuts")}
            />
          </SettingsCard>
        )}
      </SettingsSection>
    </div>
  );
}
