//! `GET /v1/doctor`: the checks behind `openside doctor`, run by the daemon, grouped like the
//! command's output, with the fix the web app can offer next to each finding.
//!
//! The checks themselves live in `openside_config::doctor` so the command and this endpoint
//! cannot drift; this module adds what only the running daemon knows (a download in progress,
//! a restart that is pending) and maps findings to fixes.

use axum::{extract::State, Json};
use chrono::Utc;
use openside_config::doctor::{self, DoctorOptions, Status, VaultProbe};
use openside_config::StoredVault;
use openside_proto::{
    DoctorCheck, DoctorFix, DoctorFixAction, DoctorGroup, DoctorReport, DoctorStatus,
    MemoryModelState,
};

use crate::{api::HttpError, engine::Shared};

/// Daemon facts the configuration crate cannot see.
#[derive(Default)]
pub(crate) struct Live {
    /// A model download running or just failed: its state and a one-line description.
    pub(crate) model: Option<(MemoryModelState, String)>,
    /// Saved settings or a downloaded model apply only after the daemon restarts.
    pub(crate) restart_pending: bool,
    /// Accepted Chromium signal exits after completed shutdowns in this daemon process.
    pub(crate) browser_shutdown_crashes: u64,
    pub(crate) chief_name_conflict: bool,
}

fn fix(action: DoctorFixAction, label: &str, hint: &str) -> Option<DoctorFix> {
    Some(DoctorFix {
        action,
        label: label.to_owned(),
        hint: hint.to_owned(),
    })
}

/// The in-app fix for a finding that is not passing, by check id.
fn fix_for(id: &str) -> Option<DoctorFix> {
    if id == "memory.embeddings" {
        return fix(
            DoctorFixAction::DownloadMemoryModel,
            "Download the model",
            "Downloads the default embedding model and checks it against its published checksums.",
        );
    }
    if id == "daemon.pending_restart" {
        return fix(
            DoctorFixAction::RestartDaemon,
            "Restart the daemon",
            "Open sessions are kept; the page reconnects by itself.",
        );
    }
    if id == "network.allow" {
        return fix(
            DoctorFixAction::OpenPolicy,
            "Open Policy",
            "Review which outside addresses openside may contact.",
        );
    }
    if id.starts_with("agent.discovery") {
        return fix(
            DoctorFixAction::OpenAgents,
            "Open the agent list",
            "See which agents were found and where their skills belong.",
        );
    }
    match id.split('.').next().unwrap_or(id) {
        "credentials" | "models" => fix(
            DoctorFixAction::OpenModels,
            "Open Models & accounts",
            "Connect a provider or add an API key.",
        ),
        "vault" => fix(
            DoctorFixAction::OpenVault,
            "Open the Vault",
            "Set the vault up or unlock it.",
        ),
        "remote" => fix(
            DoctorFixAction::OpenRemote,
            "Open Remote access",
            "Review the connections that reach this computer.",
        ),
        "config" | "configured" => fix(
            DoctorFixAction::OpenConfiguration,
            "Open Configuration",
            "Review the setting the finding names.",
        ),
        _ => None,
    }
}

fn status_of(status: Status) -> DoctorStatus {
    match status {
        Status::Ok => DoctorStatus::Ok,
        Status::Warn => DoctorStatus::Warn,
        Status::Fail => DoctorStatus::Fail,
    }
}

/// Turn the command's report into the grouped, fix-annotated one.
pub(crate) fn build(report: doctor::DoctorReport, live: &Live) -> DoctorReport {
    let mut checks: Vec<DoctorCheck> = report
        .checks
        .into_iter()
        .map(|check| {
            let mut found = DoctorCheck {
                status: status_of(check.status),
                fix: if check.status == Status::Ok {
                    None
                } else {
                    fix_for(&check.id)
                },
                id: check.id,
                summary: check.summary,
                next_step: check.next_step,
            };
            if found.id == "memory.embeddings" && check.status != Status::Ok {
                // The command's advice names a terminal command; here the button does it.
                found.next_step = Some(
                    "Download the model with the button, or run `openside memory model fetch` on the computer."
                        .to_owned(),
                );
                if let Some((state, description)) = &live.model {
                    found.summary.clone_from(description);
                    if *state != MemoryModelState::Failed {
                        found.fix = None;
                        found.next_step = None;
                    } else if let Some(retry) = found.fix.as_mut() {
                        "Try again".clone_into(&mut retry.label);
                    }
                }
            }
            found
        })
        .collect();
    if live.chief_name_conflict {
        checks.push(DoctorCheck {
            id: "agent.chief_name_conflict".to_owned(),
            status: DoctorStatus::Warn,
            summary:
                "The assistant kept its previous name because a teammate is already named openside."
                    .to_owned(),
            next_step: Some(
                "Rename that teammate, then restart to apply the assistant's default name."
                    .to_owned(),
            ),
            fix: fix(
                DoctorFixAction::OpenAgents,
                "Open the agent list",
                "Review the conflicting teammate name.",
            ),
        });
    }
    checks.push(DoctorCheck {
        id: "browser.shutdown_crashes".to_owned(),
        status: if live.browser_shutdown_crashes == 0 {
            DoctorStatus::Ok
        } else {
            DoctorStatus::Warn
        },
        summary: format!(
            "browser_shutdown_crashes={} since this daemon started",
            live.browser_shutdown_crashes
        ),
        next_step: (live.browser_shutdown_crashes != 0).then(|| {
            "Review Chromium teardown warnings. Counted shutdowns completed; imports and unfinished writes still require a successful exit.".to_owned()
        }),
        fix: None,
    });
    if live.restart_pending {
        checks.push(DoctorCheck {
            id: "daemon.pending_restart".to_owned(),
            status: DoctorStatus::Warn,
            summary: "Saved settings or a downloaded model take effect after the daemon restarts."
                .to_owned(),
            next_step: Some(
                "Restart the daemon; running sessions are kept and this page reconnects."
                    .to_owned(),
            ),
            fix: fix_for("daemon.pending_restart"),
        });
    }
    let ok = checks
        .iter()
        .all(|check| check.status != DoctorStatus::Fail);
    let groups = doctor::GROUPS
        .iter()
        .filter_map(|name| {
            let members: Vec<DoctorCheck> = checks
                .iter()
                .filter(|check| doctor::group_of(&check.id) == *name)
                .cloned()
                .collect();
            (!members.is_empty()).then(|| DoctorGroup {
                name: (*name).to_owned(),
                checks: members,
            })
        })
        .collect();
    DoctorReport {
        ok,
        generated_at: Utc::now(),
        groups,
    }
}

fn vault_probe(path: &std::path::Path) -> VaultProbe {
    VaultProbe {
        stored: match openside_vault::stored_format(path) {
            Ok(None) => Some(StoredVault::Missing),
            Ok(Some(openside_vault::StoredFormat::Passphrase)) => Some(StoredVault::Passphrase),
            Ok(Some(openside_vault::StoredFormat::Keyring)) => Some(StoredVault::Keyring),
            Err(_) => None,
        },
        keyring_reachable: openside_vault::keyring_reachable(),
    }
}

pub(crate) async fn get(State(s): State<Shared>) -> Result<Json<DoctorReport>, HttpError> {
    // The checks start a sidecar and probe the keyring; one run at a time.
    let _single_flight = s.ops.doctor.lock().await;
    let path = s
        .config
        .loaded
        .config
        .vault
        .file(&s.config.loaded.paths.data_dir);
    let probe = tokio::task::spawn_blocking(move || vault_probe(&path))
        .await
        .ok();
    let mut report = doctor::run(&s.config.loaded, DoctorOptions { vault: probe }).await;
    report
        .checks
        .retain(|check| !check.id.starts_with("credentials."));
    match crate::model_accounts::diagnostics(&s).await {
        Ok(accounts) => {
            for source in accounts.sources {
                report.checks.push(doctor::Check {
                    id: format!("credentials.{}", source.name),
                    status: if source.available {
                        Status::Ok
                    } else {
                        Status::Warn
                    },
                    summary: format!("{}: {}", source.name, source.reason),
                    next_step: (!source.available).then(|| {
                        "Open Models & accounts to check account enablement and provider pools."
                            .into()
                    }),
                });
            }
        }
        Err(_) => report.checks.push(doctor::Check {
            id: "credentials.daemon".into(),
            status: Status::Warn,
            summary: "Daemon credential metadata unavailable".into(),
            next_step: Some("Open Models & accounts and check the configured sources.".into()),
        }),
    }
    let model_restart = crate::memory_model::restart_required(&s).await;
    let live = Live {
        model: s.ops.model.headline(),
        restart_pending: model_restart || crate::settings_api::restart_required(&s).await?,
        browser_shutdown_crashes: openside_browser::browser_shutdown_crashes(),
        chief_name_conflict: s.agents.chief_name_conflict()?,
    };
    Ok(Json(build(report, &live)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use openside_config::doctor::{Check, DoctorReport as Report};

    fn check(id: &str, status: Status) -> Check {
        Check {
            id: id.to_owned(),
            status,
            summary: format!("{id} summary"),
            next_step: (status != Status::Ok).then(|| "do the thing".to_owned()),
        }
    }

    fn report(checks: Vec<Check>) -> Report {
        Report { checks }
    }

    #[test]
    fn findings_are_grouped_in_display_order_with_empty_groups_dropped() {
        let built = build(
            report(vec![
                check("vault", Status::Ok),
                check("config", Status::Ok),
                check("network.allow", Status::Ok),
                check("daemon.transport", Status::Ok),
            ]),
            &Live::default(),
        );
        let names: Vec<_> = built
            .groups
            .iter()
            .map(|group| group.name.as_str())
            .collect();
        assert_eq!(
            names,
            [
                "Daemon and config",
                "Browser and sandbox",
                "Vault",
                "Network and remote access"
            ]
        );
        assert_eq!(built.groups[0].checks.len(), 2);
        assert!(built.ok);
    }

    #[test]
    fn only_problems_get_a_fix_and_a_failure_clears_ok() {
        let built = build(
            report(vec![
                check("vault", Status::Ok),
                check("network.allow", Status::Warn),
                check("credentials.0", Status::Fail),
                check("agent.sidecar", Status::Warn),
            ]),
            &Live::default(),
        );
        let find = |id: &str| {
            built
                .groups
                .iter()
                .flat_map(|group| &group.checks)
                .find(|check| check.id == id)
                .unwrap()
        };
        assert!(find("vault").fix.is_none(), "a passing check offers no fix");
        assert_eq!(
            find("network.allow").fix.as_ref().unwrap().action,
            DoctorFixAction::OpenPolicy
        );
        assert_eq!(
            find("credentials.0").fix.as_ref().unwrap().action,
            DoctorFixAction::OpenModels
        );
        assert!(
            find("agent.sidecar").fix.is_none(),
            "no in-app fix exists for a missing sidecar"
        );
        assert!(!built.ok);
    }

    #[test]
    fn the_model_check_follows_the_download_and_a_pending_restart_is_added() {
        let embeddings = || report(vec![check("memory.embeddings", Status::Warn)]);
        let only = |built: &DoctorReport| {
            built
                .groups
                .iter()
                .flat_map(|group| &group.checks)
                .find(|check| check.id == "memory.embeddings")
                .unwrap()
                .clone()
        };

        let idle = only(&build(embeddings(), &Live::default()));
        assert_eq!(
            idle.fix.unwrap().action,
            DoctorFixAction::DownloadMemoryModel
        );
        assert!(idle
            .next_step
            .unwrap()
            .contains("Download the model with the button"));

        let running = only(&build(
            embeddings(),
            &Live {
                model: Some((
                    MemoryModelState::Downloading,
                    "downloading the embedding model (42%)".into(),
                )),
                restart_pending: false,
                ..Live::default()
            },
        ));
        assert_eq!(running.summary, "downloading the embedding model (42%)");
        assert!(running.fix.is_none() && running.next_step.is_none());

        let failed = only(&build(
            embeddings(),
            &Live {
                model: Some((
                    MemoryModelState::Failed,
                    "the last model download failed: offline".into(),
                )),
                restart_pending: false,
                ..Live::default()
            },
        ));
        assert_eq!(failed.fix.unwrap().label, "Try again");

        let pending = build(
            report(vec![check("config", Status::Ok)]),
            &Live {
                restart_pending: true,
                ..Live::default()
            },
        );
        let restart = &pending.groups[0].checks.last().unwrap();
        assert_eq!(restart.id, "daemon.pending_restart");
        assert_eq!(
            restart.fix.as_ref().unwrap().action,
            DoctorFixAction::RestartDaemon
        );
    }

    #[test]
    fn completed_shutdown_crash_counter_is_visible_without_failing_doctor() {
        for count in [0, 7] {
            let built = build(
                report(Vec::new()),
                &Live {
                    browser_shutdown_crashes: count,
                    ..Live::default()
                },
            );
            let check = built
                .groups
                .iter()
                .flat_map(|group| &group.checks)
                .find(|check| check.id == "browser.shutdown_crashes")
                .unwrap();
            assert_eq!(
                check.status,
                if count == 0 {
                    DoctorStatus::Ok
                } else {
                    DoctorStatus::Warn
                }
            );
            assert!(check
                .summary
                .contains(&format!("browser_shutdown_crashes={count} ")));
            assert_eq!(check.next_step.is_some(), count != 0);
            assert!(check.fix.is_none());
            assert!(built.ok);
        }
    }
    #[test]
    fn chief_name_collision_is_actionable_without_failing_doctor() {
        let built = build(
            report(Vec::new()),
            &Live {
                chief_name_conflict: true,
                ..Live::default()
            },
        );
        let check = built
            .groups
            .iter()
            .flat_map(|group| &group.checks)
            .find(|check| check.id == "agent.chief_name_conflict")
            .unwrap();
        assert_eq!(check.status, DoctorStatus::Warn);
        assert_eq!(
            check.fix.as_ref().unwrap().action,
            DoctorFixAction::OpenAgents
        );
        assert!(check.next_step.is_some());
        assert!(built.ok);
    }
}
