import { CircleCheck, RefreshCw } from "lucide-react";
import { useCallback, useState } from "react";
import { useLocation, useNavigate } from "react-router";
import type { DoctorCheck, DoctorFixAction, DoctorReport } from "../api/types";
import { useClient } from "../context";
import { errorMessage, useResource } from "../hooks/useResource";
import { Button, ICON_STROKE, useToast } from "../ui";
import { ErrorNote, ResourceCard, ResourceView, SectionList, type BadgeTone } from "./blocks";
import { formatDateTime } from "./format";
import { RestartStatus, useDaemonRestart } from "./RestartNotice";

const STATUS: Record<DoctorCheck["status"], { label: string; tone: BadgeTone }> = {
  ok: { label: "OK", tone: "success" },
  warn: { label: "Warning", tone: "attention" },
  fail: { label: "Problem", tone: "danger" },
};

/** Where each "open …" fix goes; the other actions do something here first. */
const DESTINATION: Partial<Record<DoctorFixAction, string>> = {
  open_models: "/models",
  open_vault: "/vault",
  open_remote: "/remote",
  open_configuration: "/config",
  open_agents: "/agents",
  open_policy: "/policy",
};

function Summary({ report }: { report: DoctorReport }) {
  const checks = report.groups.flatMap((group) => group.checks);
  const problems = checks.filter((check) => check.status === "fail").length;
  const warnings = checks.filter((check) => check.status === "warn").length;
  if (problems === 0 && warnings === 0) {
    return (
      <p className="diagnostics-summary" data-state="ok" role="status">
        <CircleCheck size={18} strokeWidth={ICON_STROKE} aria-hidden="true" /> Everything checks out.
      </p>
    );
  }
  const parts = [
    problems ? `${problems} ${problems === 1 ? "problem" : "problems"}` : null,
    warnings ? `${warnings} ${warnings === 1 ? "warning" : "warnings"}` : null,
  ].filter(Boolean);
  return (
    <p className="diagnostics-summary" data-state={problems ? "fail" : "warn"} role="status">
      {parts.join(" and ")} to look at.
    </p>
  );
}

/** What openside found wrong on this computer, grouped by area, with a button that fixes what it can. */
export function DiagnosticsPage() {
  const client = useClient();
  const toast = useToast();
  const navigate = useNavigate();
  const location = useLocation();
  const load = useCallback(() => client.doctor(), [client]);
  const report = useResource(load);
  const [running, setRunning] = useState(false);
  const [fixing, setFixing] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const restart = useDaemonRestart({ onRestarted: () => void report.reload() });

  async function again() {
    setRunning(true);
    await report.reload();
    setRunning(false);
  }

  async function fix(check: DoctorCheck) {
    const action = check.fix?.action;
    if (!action) return;
    setError(null);
    const destination = DESTINATION[action];
    if (destination) {
      navigate(destination, { replace: true, state: location.state });
    } else if (action === "restart_daemon") {
      restart.ask();
    } else if (action === "download_memory_model") {
      setFixing(check.id);
      try {
        await client.downloadMemoryModel();
        toast.notify("Download started.");
        // The Memory page shows the progress and the cancel button.
        navigate("/memory", { replace: true, state: location.state });
      } catch (cause) {
        setError(errorMessage(cause));
      } finally {
        setFixing(null);
      }
    }
  }

  return (
    <div className="manage-body" data-section="diagnostics">
      <ResourceView resource={report} what="diagnostics">
        {({ generated_at, groups }) => (
          <>
            <SectionList
              summary={`Checked ${formatDateTime(generated_at)}`}
              actions={
                <Button size="compact" disabled={running} onClick={() => void again()}>
                  <RefreshCw size={16} strokeWidth={ICON_STROKE} aria-hidden="true" />
                  {running ? "Checking…" : "Check again"}
                </Button>
              }
            >
              {report.data ? <Summary report={report.data} /> : null}
            </SectionList>
            <RestartStatus restart={restart} />
            {error ? <ErrorNote>{error}</ErrorNote> : null}
            {groups.map((group) => (
              <SectionList key={group.name} title={group.name}>
                <ul className="resource-list" aria-label={group.name}>
                  {group.checks.map((check) => {
                    const status = STATUS[check.status];
                    return (
                      <li key={check.id}>
                        <ResourceCard
                          data-check={check.id}
                          title={check.summary}
                          detail={[check.next_step, check.fix?.hint].filter(Boolean).join(" ") || undefined}
                          badge={status.label}
                          badgeTone={status.tone}
                          trailing={
                            check.fix ? (
                              <Button
                                size="compact"
                                variant={check.status === "fail" ? "primary" : "secondary"}
                                disabled={fixing !== null || restart.phase === "restarting"}
                                aria-label={`${check.fix.label}: ${check.summary}`}
                                onClick={() => void fix(check)}
                              >
                                {fixing === check.id ? "Working…" : check.fix.label}
                              </Button>
                            ) : undefined
                          }
                        />
                      </li>
                    );
                  })}
                </ul>
              </SectionList>
            ))}
          </>
        )}
      </ResourceView>
    </div>
  );
}
