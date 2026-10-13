import type { ImportBatchReport, ImportCategory, ImportCategoryReport, ImportJob, ImportProfile } from "../api/types";

export interface Kind {
  id: ImportCategory;
  label: string;
  one: string;
  many: string;
}

/** Display order of the kinds an import can carry. */
export const KINDS: readonly Kind[] = [
  { id: "history", label: "History", one: "history entry", many: "history entries" },
  { id: "bookmarks", label: "Bookmarks", one: "bookmark", many: "bookmarks" },
  { id: "cookies", label: "Cookies", one: "cookie", many: "cookies" },
  { id: "passwords", label: "Passwords", one: "password", many: "passwords" },
  { id: "autofill", label: "Autofill", one: "autofill entry", many: "autofill entries" },
  { id: "tabs", label: "Open tabs", one: "open tab", many: "open tabs" },
  { id: "search_engines", label: "Search engines", one: "search engine", many: "search engines" },
  { id: "permissions", label: "Site permissions", one: "site permission", many: "site permissions" },
  { id: "extensions", label: "Extensions", one: "extension", many: "extensions" },
];

export const ALL_KINDS: readonly ImportCategory[] = KINDS.map((kind) => kind.id);

interface Brand {
  label: string;
  /** Tile colour behind the initial; the card is dark in both themes. */
  tint: string;
}

const BRANDS: Record<string, Brand> = {
  chrome: { label: "Chrome", tint: "#4c8bf5" },
  chromium: { label: "Chromium", tint: "#3f7fd8" },
  brave: { label: "Brave", tint: "#e0562a" },
  edge: { label: "Edge", tint: "#1f9d8f" },
  vivaldi: { label: "Vivaldi", tint: "#d4393f" },
  opera: { label: "Opera", tint: "#d63a4a" },
  arc: { label: "Arc", tint: "#7a5cf0" },
  aside: { label: "Aside", tint: "#5a6a85" },
  firefox: { label: "Firefox", tint: "#e8742c" },
  librewolf: { label: "LibreWolf", tint: "#3b8fd0" },
  zen: { label: "Zen", tint: "#c46a52" },
  floorp: { label: "Floorp", tint: "#3f78d8" },
  safari: { label: "Safari", tint: "#2b9bd8" },
};

export function brandOf(profile: Pick<ImportProfile, "brand" | "browser">): Brand & { initial: string } {
  const known = BRANDS[profile.brand] ?? BRANDS[profile.browser];
  const label = known?.label ?? profile.brand;
  return { label, tint: known?.tint ?? "#6b7488", initial: label.slice(0, 1).toUpperCase() };
}

export const profileTitle = (profile: ImportProfile) => `${brandOf(profile).label} · ${profile.name}`;

export const formatCount = (value: number) => value.toLocaleString("en-US");

export function kindOf(id: ImportCategory): Kind {
  return KINDS.find((kind) => kind.id === id)!;
}

export function phrase(id: ImportCategory, value: number): string {
  const kind = kindOf(id);
  return `${formatCount(value)} ${value === 1 ? kind.one : kind.many}`;
}

export type Counts = Partial<Record<ImportCategory, number>>;

/** Prospective per-kind counts of one profile in a preview; kinds nothing was found for are left out. */
export function profileCounts(report: ImportBatchReport | null, profileId: string): Counts {
  const entry = report?.profiles.find((candidate) => candidate.id === profileId);
  const counts: Counts = {};
  if (!entry) return counts;
  for (const id of ALL_KINDS) {
    const category: ImportCategoryReport | null | undefined = entry[id];
    if (category && category.imported > 0) counts[id] = category.imported;
  }
  return counts;
}

export function sumCounts(counts: Counts[], kinds: ReadonlySet<ImportCategory>): Counts {
  const total: Counts = {};
  for (const part of counts) {
    for (const id of kinds) {
      const value = part[id];
      if (value) total[id] = (total[id] ?? 0) + value;
    }
  }
  return total;
}

export const totalOf = (counts: Counts) => Object.values(counts).reduce((sum, value) => sum + (value ?? 0), 0);

/** `12,000 history entries · 300 bookmarks`; the kinds that have something, in display order. */
export function describeCounts(counts: Counts): string {
  return KINDS.filter(({ id }) => counts[id])
    .map(({ id }) => phrase(id, counts[id]!))
    .join(" · ");
}

/** Every profile and kind selected: the request leaves both out, which the daemon reads as "everything". */
export function requestFor(profiles: ReadonlySet<string>, all: readonly ImportProfile[], kinds: ReadonlySet<ImportCategory>) {
  const everyProfile = all.every((profile) => profiles.has(profile.id));
  const everyKind = ALL_KINDS.every((id) => kinds.has(id));
  return {
    profiles: everyProfile ? null : [...profiles],
    what: everyKind ? null : [...kinds],
  };
}

export function progressOf(job: ImportJob): { done: number; total: number; ratio: number } {
  const total = Math.max(job.total_profiles, 1);
  return { done: job.completed_profiles, total: job.total_profiles, ratio: Math.min(job.completed_profiles / total, 1) };
}

/** The daemon's own warnings are already worded per profile and kind; per-kind notes are only a fallback when it sent none. */
export function outcomeNotes(report: ImportBatchReport): string[] {
  if (report.warnings.length > 0) return [...new Set(report.warnings)];
  const notes: string[] = [];
  for (const entry of report.profiles) {
    for (const id of ALL_KINDS) {
      const note = entry[id]?.note;
      if (note && !notes.includes(note)) notes.push(note);
    }
  }
  return notes;
}
