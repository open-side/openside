import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it } from "vitest";
import { accentForeground, applyAccent, applyPalettes, applyTheme, loadAccent, loadPalettes, loadTheme } from "../theme";
import { AppearanceSettings } from "./AppearanceSettings";

const root = document.documentElement;

afterEach(() => {
  localStorage.clear();
  applyTheme("system");
  applyAccent("");
  applyPalettes({ light: "openside", dark: "openside" });
});

describe("AppearanceSettings", () => {
  it("switches system → light → dark → system and persists explicit choices", async () => {
    const user = userEvent.setup();
    applyTheme(loadTheme());
    render(<AppearanceSettings />);
    const mode = screen.getByRole("combobox", { name: "Theme mode" });
    expect(root.dataset.theme).toBe("system");
    expect(mode).toHaveValue("system");

    await user.selectOptions(mode, "light");
    expect(root.dataset.theme).toBe("light");
    expect(localStorage.getItem("openside.theme")).toBe("light");

    await user.selectOptions(mode, "dark");
    expect(root.dataset.theme).toBe("dark");
    expect(loadTheme()).toBe("dark");

    await user.selectOptions(mode, "system");
    expect(root.dataset.theme).toBe("system");
    expect(localStorage.getItem("openside.theme")).toBeNull();
  });

  it("applies a palette per scheme and stores its colours for the boot script", async () => {
    const user = userEvent.setup();
    render(<AppearanceSettings />);
    await user.selectOptions(screen.getByRole("combobox", { name: "Dark theme" }), "dracula-dark");
    expect(root.dataset.darkPalette).toBe("dracula-dark");
    expect(root.style.getPropertyValue("--dark-canvas")).toBe("#282a36");
    expect(root.style.getPropertyValue("--dark-accent-fg")).toBe(accentForeground("#ff79c6"));
    expect(root.dataset.lightPalette).toBeUndefined();
    expect(JSON.parse(localStorage.getItem("openside.palette") ?? "{}")).toEqual({
      dark: { id: "dracula-dark", background: "#282a36", foreground: "#f8f8f2", accent: "#ff79c6" },
    });
    expect(loadPalettes()).toEqual({ light: "openside", dark: "dracula-dark" });
    // A light preset is never offered for the dark scheme.
    expect(screen.getByRole("combobox", { name: "Dark theme" }).querySelector('option[value="codex-light"]')).toBeNull();

    await user.selectOptions(screen.getByRole("combobox", { name: "Dark theme" }), "openside");
    expect(root.dataset.darkPalette).toBeUndefined();
    expect(root.style.getPropertyValue("--dark-canvas")).toBe("");
    expect(localStorage.getItem("openside.palette")).toBeNull();
  });

  it("overrides the palette accent with a swatch and restores it with Theme default", async () => {
    const user = userEvent.setup();
    render(<AppearanceSettings />);
    const reset = screen.getByRole("button", { name: "Theme default" });
    expect(reset).toBeDisabled();

    await user.click(screen.getByRole("button", { name: "Yellow" }));
    expect(root.style.getPropertyValue("--primary")).toBe("#f0b429");
    expect(root.style.getPropertyValue("--primary-foreground")).toBe("#000000");
    expect(loadAccent()).toBe("#f0b429");
    expect(screen.getByRole("button", { name: "Yellow" })).toHaveAttribute("aria-pressed", "true");

    await user.click(reset);
    expect(root.style.getPropertyValue("--primary")).toBe("");
    expect(localStorage.getItem("openside.accent")).toBeNull();
    expect(screen.getByRole("button", { name: "Yellow" })).toHaveAttribute("aria-pressed", "false");
  });

  it("ignores invalid stored values", () => {
    localStorage.setItem("openside.theme", "neon");
    expect(loadTheme()).toBe("system");
    localStorage.setItem("openside.accent", "red; color: x");
    expect(loadAccent()).toBe("");
    localStorage.setItem("openside.palette", JSON.stringify({ light: { id: "dracula-dark" }, dark: { id: "nope" } }));
    expect(loadPalettes()).toEqual({ light: "openside", dark: "openside" });
    localStorage.setItem("openside.palette", "{not json");
    expect(loadPalettes()).toEqual({ light: "openside", dark: "openside" });
    expect(accentForeground("#ffffff")).toBe("#000000");
  });
});
