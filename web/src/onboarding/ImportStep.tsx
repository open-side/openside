import { Check, Globe } from "lucide-react";
import { useCallback, useEffect, useMemo, useState } from "react";
import { ApiError } from "../api/client";
import type { ImportBrowser, ImportCategory, ImportCategoryReport, ImportProfile, ImportReport } from "../api/types";
import { useClient } from "../context";
import { errorMessage, useResource } from "../hooks/useResource";
import { Badge, EmptyState, ErrorNote, ResourceView } from "../pages/blocks";
import { Button, ICON_STROKE, Spinner } from "../ui";
import type { StepProps } from "./summary";

const BROWSER: Record<ImportBrowser, string> = { chrome: "Chrome", chromium: "Chromium", firefox: "Firefox" };

const CATEGORIES: { id: ImportCategory; label: string; one: string; many: string }[] = [
  { id: "history", label: "History", one: "history entry", many: "history entries" },
  { id: "bookmarks", label: "Bookmarks", one: "bookmark", many: "bookmarks" },
  { id: "cookies", label: "Cookies", one: "cookie", many: "cookies" },
];

/** Failures that only concern cookies: the other categories can still go through. */
const COOKIE_ONLY = new Set(["keyring_password_required", "profile_in_use", "browser_unavailable"]);

const profileName = (profile: ImportProfile) => `${BROWSER[profile.browser]} · ${profile.name}`;
const count = (value: number) => value.toLocaleString("en-US");

function reportFor(report: ImportReport, id: ImportCategory): ImportCategoryReport | null {
  return report[id] ?? null;
}

function Outcome({ report, written }: { report: ImportReport; written: boolean }) {
  return (
    <ul className="onb-counts" aria-label={written ? "Imported" : "Preview"}>
      {CATEGORIES.map(({ id, label, one, many }) => {
        const category = reportFor(report, id);
        if (!category) return null;
        return (
          <li key={id}>
            <span>{label}</span>
            <span className="onb-counts-value">
              {category.note
                ? category.note
                : `${count(category.imported)} ${written ? "imported" : `${category.imported === 1 ? one : many} to import`}${category.skipped ? ` · ${count(category.skipped)} skipped` : ""}${category.rejected.length ? ` · ${count(category.rejected.length)} refused` : ""}`}
            </span>
          </li>
        );
      })}
    </ul>
  );
}

export function ImportStep({ update }: StepProps) {
  const client = useClient();
  const load = useCallback(() => client.importProfiles(), [client]);
  const profiles = useResource(load);
  const [profileId, setProfileId] = useState<string | null>(null);
  const [picked, setPicked] = useState<ReadonlySet<ImportCategory>>(new Set());
  const [busy, setBusy] = useState<"preview" | "import" | null>(null);
  const [error, setError] = useState<{ message: string; code: string | null } | null>(null);
  const [preview, setPreview] = useState<{ key: string; report: ImportReport } | null>(null);
  const [written, setWritten] = useState<{ key: string; report: ImportReport } | null>(null);

  const list = profiles.data?.profiles;
  const cookiesWritable = profiles.data?.cookies_writable ?? false;
  const profile = list?.find((candidate) => candidate.id === profileId) ?? null;
  const usable = useCallback(
    (candidate: ImportProfile, category: ImportCategory) =>
      candidate.available[category] && (category !== "cookies" || cookiesWritable),
    [cookiesWritable],
  );

  const choose = useCallback(
    (candidate: ImportProfile) => {
      setProfileId(candidate.id);
      setPicked(new Set(CATEGORIES.filter(({ id }) => usable(candidate, id)).map(({ id }) => id)));
      setError(null);
    },
    [usable],
  );

  useEffect(() => {
    if (list && profileId === null && list.length > 0) choose(list.find((candidate) => CATEGORIES.some(({ id }) => usable(candidate, id))) ?? list[0]!);
  }, [list, profileId, choose, usable]);

  const key = useMemo(() => `${profileId}|${[...picked].sort().join(",")}`, [profileId, picked]);
  const previewed = preview?.key === key;
  const done = written?.key === key;

  function toggle(category: ImportCategory) {
    setPicked((current) => {
      const next = new Set(current);
      if (!next.delete(category)) next.add(category);
      return next;
    });
    setError(null);
  }

  async function run(dryRun: boolean) {
    if (!profile) return;
    setBusy(dryRun ? "preview" : "import");
    setError(null);
    try {
      const report = await client.runImport({ profile: profile.id, what: [...picked], dry_run: dryRun });
      if (dryRun) setPreview({ key, report });
      else {
        setWritten({ key, report });
        update({ imported: profileName(profile) });
      }
    } catch (cause) {
      setError({ message: errorMessage(cause), code: cause instanceof ApiError ? cause.code : null });
    } finally {
      setBusy(null);
    }
  }

  return (
    <ResourceView resource={profiles} what="browser import">
      {({ profiles: all }) =>
        all.length === 0 ? (
          <EmptyState
            icon={Globe}
            title="No browser profiles found"
            detail={
              <>
                openside looks for Chrome, Chromium and Firefox in their usual folders on this computer. For another location, use{" "}
                <code>openside import</code> in a terminal.
              </>
            }
          />
        ) : (
          <>
            <div role="radiogroup" aria-label="Browser profile" className="onb-choices">
              {all.map((candidate) => (
                <label key={candidate.id} className="onb-choice" data-checked={candidate.id === profileId || undefined}>
                  <input type="radio" name="profile" checked={candidate.id === profileId} onChange={() => choose(candidate)} />
                  <span className="onb-choice-copy">
                    <strong>{profileName(candidate)}</strong>
                    <span>
                      {CATEGORIES.filter(({ id }) => candidate.available[id])
                        .map(({ label }) => label)
                        .join(" · ") || "Nothing to import"}
                    </span>
                  </span>
                </label>
              ))}
            </div>
            {profile ? (
              <fieldset className="onb-fieldset">
                <legend>What to bring</legend>
                {CATEGORIES.map(({ id, label }) => (
                  <label key={id} className="onb-check">
                    <input type="checkbox" checked={picked.has(id)} disabled={!usable(profile, id) || busy !== null} onChange={() => toggle(id)} />
                    <span>{label}</span>
                    {!profile.available[id] ? (
                      <Badge>Not in this profile</Badge>
                    ) : id === "cookies" && !cookiesWritable ? (
                      <Badge>Needs a Chromium browser</Badge>
                    ) : null}
                  </label>
                ))}
              </fieldset>
            ) : null}
            {picked.has("cookies") ? (
              <p className="onb-note">
                Cookies keep the agent signed in to your sites. They are copied into openside’s own browser profile on this computer and
                nowhere else. The original browser is only read.
              </p>
            ) : null}
            {error ? (
              <div className="onb-problem">
                <ErrorNote>{error.message}</ErrorNote>
                {error.code && COOKIE_ONLY.has(error.code) && picked.has("cookies") ? (
                  <Button size="compact" onClick={() => toggle("cookies")}>
                    Leave cookies out
                  </Button>
                ) : null}
              </div>
            ) : null}
            {done && written ? (
              <div className="onb-result" role="status">
                <p className="onb-result-title">
                  <Check size={16} strokeWidth={ICON_STROKE} aria-hidden="true" /> Imported from {profileName(profile!)}
                </p>
                <Outcome report={written.report} written />
              </div>
            ) : previewed && preview ? (
              <div className="onb-result" role="status">
                <p className="onb-result-title">Preview — nothing has been written yet</p>
                <Outcome report={preview.report} written={false} />
              </div>
            ) : null}
            {!done ? (
              <div className="onb-actions">
                <Button disabled={picked.size === 0 || busy !== null} onClick={() => void run(true)}>
                  {busy === "preview" ? (
                    <>
                      <Spinner size={16} /> Reading…
                    </>
                  ) : previewed ? (
                    "Preview again"
                  ) : (
                    "Preview"
                  )}
                </Button>
                <Button variant="primary" disabled={!previewed || busy !== null} onClick={() => void run(false)}>
                  {busy === "import" ? (
                    <>
                      <Spinner size={16} /> Importing…
                    </>
                  ) : (
                    "Import"
                  )}
                </Button>
              </div>
            ) : null}
          </>
        )
      }
    </ResourceView>
  );
}
