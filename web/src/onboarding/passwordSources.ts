import type { ImportProfile } from "../api/types";
import { t, type MessageKey } from "../i18n";
import { brandOf } from "./importModel";

/**
 * Where a CSV of passwords comes from, and how to get it out of there. `steps` is a sentence that
 * takes `{name}`; `menu` is the path drawn in the sheet's illustration. The menu items are the
 * exporting app's own labels, kept verbatim because the illustration mirrors that app, while the
 * translated `steps` sentence carries the instructions.
 */
export interface PasswordSource {
  id: string;
  name: string;
  steps: MessageKey;
  menu: string[];
  /** Tile colour behind the initial, like the browser tiles of importModel. */
  tint: string;
}

const CHROMIUM = { steps: "onboarding.source.chromium", menu: ["Settings", "Passwords", "Export passwords"] } as const;
const FIREFOX = { steps: "onboarding.source.firefox", menu: ["Passwords", "•••", "Export passwords…"] } as const;
const APPLE = { steps: "onboarding.source.apple", menu: ["File", "Export All Passwords to File…"] } as const;

/** The password managers listed under "Other password managers"; a function so the last name follows the current locale. */
export function passwordSources(): readonly PasswordSource[] {
  return [
    { id: "bitwarden", name: "Bitwarden", steps: "onboarding.source.bitwarden", menu: ["File", "Export vault", "File format: .csv"], tint: "#175ddc" },
    { id: "1password", name: "1Password", steps: "onboarding.source.onePassword", menu: ["File", "Export", "CSV"], tint: "#0a6ef0" },
    { id: "lastpass", name: "LastPass", steps: "onboarding.source.lastpass", menu: ["Advanced options", "Export"], tint: "#d32d27" },
    { id: "apple", name: "Apple Passwords", steps: APPLE.steps, menu: [...APPLE.menu], tint: "#2f9e5b" },
    { id: "other", name: t("onboarding.source.otherName"), steps: "onboarding.source.other", menu: ["File", "Export", "CSV"], tint: "#6b7488" },
  ];
}

/** A detected browser exports with its family's steps (Chromium, Firefox or Apple), under its own name. */
export function sourceForProfile(profile: Pick<ImportProfile, "id" | "brand" | "browser">): PasswordSource {
  const family = profile.browser === "safari" ? APPLE : profile.browser === "firefox" ? FIREFOX : CHROMIUM;
  const brand = brandOf(profile);
  return { id: profile.id, name: brand.label, steps: family.steps, menu: [...family.menu], tint: brand.tint };
}
