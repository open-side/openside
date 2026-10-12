import { ChevronDown } from "lucide-react";
import { useState, type CSSProperties, type ReactNode } from "react";
import {
  ACCENT_COLORS,
  applyAccent,
  applyPalettes,
  applyTheme,
  DEFAULT_ACCENT,
  DEFAULT_PALETTE,
  loadAccent,
  loadPalettes,
  loadTheme,
  presetsFor,
  resolvedTheme,
  type Palettes,
  type ThemePreference,
} from "../theme";
import { ICON_STROKE } from "../ui";

const MODES: { value: ThemePreference; label: string }[] = [
  { value: "system", label: "System" },
  { value: "light", label: "Light" },
  { value: "dark", label: "Dark" },
];

function SelectRow({ id, label, value, onChange, children }: { id: string; label: string; value: string; onChange: (value: string) => void; children: ReactNode }) {
  return (
    <div className="appearance-row">
      <label htmlFor={id}>{label}</label>
      <span className="appearance-select">
        <select id={id} value={value} onChange={(event) => onChange(event.target.value)}>
          {children}
        </select>
        <ChevronDown size={16} strokeWidth={ICON_STROKE} aria-hidden="true" />
      </span>
    </div>
  );
}

/**
 * Theme mode, a palette per scheme and the primary colour (the aside-relay Appearance
 * settings, minus web fonts, which openside does not fetch). Stored in this browser only.
 */
export function AppearanceSettings() {
  const [theme, setTheme] = useState<ThemePreference>(loadTheme);
  const [palettes, setPalettes] = useState<Palettes>(loadPalettes);
  const [accent, setAccent] = useState(loadAccent);
  const scheme = resolvedTheme(theme);
  const paletteAccent = presetsFor(scheme).find((preset) => preset.id === palettes[scheme])?.accent ?? DEFAULT_ACCENT;

  const choosePalette = (which: keyof Palettes, id: string) => {
    const next = { ...palettes, [which]: id };
    applyPalettes(next);
    setPalettes(next);
  };
  const chooseAccent = (color: string) => {
    applyAccent(color);
    setAccent(color);
  };

  return (
    <div className="appearance settings-group">
      <SelectRow
        id="appearance-mode"
        label="Theme mode"
        value={theme}
        onChange={(value) => {
          const next = value as ThemePreference;
          applyTheme(next);
          setTheme(next);
        }}
      >
        {MODES.map((mode) => (
          <option key={mode.value} value={mode.value}>
            {mode.label}
          </option>
        ))}
      </SelectRow>
      {(["light", "dark"] as const).map((which) => (
        <SelectRow
          key={which}
          id={`appearance-${which}`}
          label={which === "light" ? "Light theme" : "Dark theme"}
          value={palettes[which]}
          onChange={(id) => choosePalette(which, id)}
        >
          <option value={DEFAULT_PALETTE}>openside</option>
          {presetsFor(which).map((preset) => (
            <option key={preset.id} value={preset.id}>
              {preset.name}
            </option>
          ))}
        </SelectRow>
      ))}
      <div className="appearance-row">
        <label htmlFor="appearance-accent">Primary color</label>
        <span className="appearance-accent">
          <button type="button" className="appearance-reset" disabled={!accent} onClick={() => chooseAccent("")}>
            Theme default
          </button>
          <input id="appearance-accent" type="color" value={accent || paletteAccent} onChange={(event) => chooseAccent(event.target.value)} />
        </span>
      </div>
      <div className="accent-swatches" role="group" aria-label="Primary color presets">
        {ACCENT_COLORS.map((color) => (
          <button
            key={color.value}
            type="button"
            aria-pressed={accent === color.value}
            aria-label={color.name}
            title={color.name}
            className="accent-swatch"
            style={{ "--swatch": color.value } as CSSProperties}
            onClick={() => chooseAccent(color.value)}
          />
        ))}
      </div>
    </div>
  );
}
