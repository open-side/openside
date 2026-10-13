import { Monitor, Moon, Sun } from "lucide-react";
import { useState, type CSSProperties } from "react";
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
import { ICON_STROKE, Select, type SelectOption } from "../ui";

const MODE_ICON = { size: 16, strokeWidth: ICON_STROKE } as const;
const MODES: SelectOption[] = [
  { value: "system", label: "System", icon: <Monitor {...MODE_ICON} /> },
  { value: "light", label: "Light", icon: <Sun {...MODE_ICON} /> },
  { value: "dark", label: "Dark", icon: <Moon {...MODE_ICON} /> },
];

// The built-in palette has no preset entry; these are its canvas, text and accent from styles.css.
const DEFAULT_SWATCH = {
  light: { background: "#ffffff", foreground: "#171717", accent: DEFAULT_ACCENT },
  dark: { background: "#000000", foreground: "#f3f3f3", accent: DEFAULT_ACCENT },
} as const;

/** A palette preview: its canvas with an "Aa" in its text colour and a dot in its accent. */
function ThemeSwatch({ background, foreground, accent }: { background: string; foreground: string; accent: string }) {
  return (
    <span className="theme-swatch" style={{ "--swatch-bg": background, "--swatch-fg": foreground, "--swatch-accent": accent } as CSSProperties}>
      Aa
    </span>
  );
}

function SelectRow({ id, label, value, options, onChange }: { id: string; label: string; value: string; options: SelectOption[]; onChange: (value: string) => void }) {
  return (
    <div className="appearance-row">
      <label htmlFor={id}>{label}</label>
      <Select id={id} variant="inline" value={value} options={options} onChange={onChange} />
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
        options={MODES}
        onChange={(value) => {
          const next = value as ThemePreference;
          applyTheme(next);
          setTheme(next);
        }}
      />
      {(["light", "dark"] as const).map((which) => (
        <SelectRow
          key={which}
          id={`appearance-${which}`}
          label={which === "light" ? "Light theme" : "Dark theme"}
          value={palettes[which]}
          options={[
            { value: DEFAULT_PALETTE, label: "openside", icon: <ThemeSwatch {...DEFAULT_SWATCH[which]} /> },
            ...presetsFor(which).map((preset) => ({
              value: preset.id,
              label: preset.name,
              icon: <ThemeSwatch background={preset.background} foreground={preset.foreground} accent={preset.accent} />,
            })),
          ]}
          onChange={(id) => choosePalette(which, id)}
        />
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
