# GOAL 3 end-to-end scenarios

Run `tools/e2e-scenarios.sh` from any directory. This table is the only scenario test list.
The runner issues one `cargo test -j 8 -p <crate>` invocation per crate, selects only these
integration-test targets, and excludes every unlisted test in those targets. A missing test,
build failure, failed test, ignored test, or printed skip makes the suite fail. It prints the
full Cargo output and a scenario/test/PASS-or-FAIL table, and saves both to
`target/e2e-scenarios.log`. No gate integration is implied.
`--no-fail-fast` keeps the remaining scenario targets running after an individual failure.

The tests use real daemon processes, private HOME/XDG/data directories, the real Python
sidecar with a scripted FunctionModel, real Chromium for browser scenarios, and loopback
fixtures rather than real services or credentials. Install the repository's Python
environment and build the web app before running. Chromium is discovered through the
existing browser configuration/environment/PATH rules. Missing prerequisites are not PASS
in this completion suite, even when the individual portable test explains a skip.

The process harness requires Unix. The active-turn browser SIGKILL scenario and optional
compositor IPC test are Linux-only. On a platform where a listed test is not compiled or
executed, the completion runner fails rather than silently reporting PASS.

| Scenario | Crate | Target | Test |
| --- | --- | --- | --- |
| 1 Login, 2FA ticket, daemon restart, resume | openside-e2e | human_step | hs1_vault_login_secret_ticket_survives_daemon_restart_without_canary_leaks |
| 2 Checkout payment lure blocked without approval | openside-e2e | scenarios | fixture_shop_checkout_lure_is_blocked_without_human_approval |
| 3 Child approval routed to parent | openside-e2e | subagents | child_approval_lands_in_parent_tickets_and_resumes_child_then_parent |
| 4 Routine run | openside-e2e | routines | run_now_from_the_cli_starts_a_real_session_and_the_run_completes |
| 4 Routine run | openside-e2e | routines | a_due_routine_fires_by_itself_and_a_restart_records_what_it_missed |
| 5 Memory recall | openside-e2e | memory | memory_written_in_one_session_is_found_by_a_later_session |
| 6 Channel bot round trip | openside-e2e | channels | telegram_thread_is_a_session_commands_work_and_mapping_survives_bot_restart |
| 7 Web UI approval and ticket resolution | openside-e2e | web_ui | web_ui_subagent_view_and_approval_resume_same_call_and_reload_rebuilds_screen |
| 7 Web UI approval and ticket resolution | openside-e2e | web_ui | web_ui_secret_input_and_takeover_screen_resume_the_agent |
| 8 Push receipt | openside-e2e | remote | phone_pairs_by_qr_is_pushed_answers_from_the_deep_link_and_hears_the_task_finish |
| 8 Push receipt | openside-e2e | push_browser | chromium_service_worker_shows_the_pushed_ticket_with_its_deep_link |
| 9 Sidecar and browser SIGKILL recovery | openside-e2e | daemon | sidecar_sigkill_restores_snapshot_preserves_queue_and_completes_each_id_once |
| 9 Sidecar and browser SIGKILL recovery | openside-e2e | scenarios | browser_sigkill_during_active_task_recovers_without_resubmitting_or_restarting_sidecar |
| 9 Sidecar and browser SIGKILL recovery | openside-e2e | scenarios | task_status_and_sleep_inhibitor_stay_running_through_multiple_browser_tools |
| 10 Two-instance sync | openside-e2e | sync | two_devices_sync_settings_pins_vault_and_memories_e2e_encrypted |

## Coverage audit

1. The existing login test already performs a real password login, opens a secret-input 2FA
   ticket, crashes/relaunches the daemon while that ticket is pending, resolves it through
   the CLI, and observes successful verification at the fixture server. It checks a single
   resumed tool return/resolution and no password/code canaries in public storage or logs.
   No duplicate login test is needed.
2. The existing red-team inbox test proves payment/send/exfiltration denial. The new shop
   test additionally follows a real checkout link, reads the hostile checkout instruction
   into the model's committed conversation, executes a safe positive-control click, and
   attempts the injected payment. Policy denies it, the payment endpoint receives zero
   calls, and no human approval is requested or supplied.
3. The parent-routing test checks the child's origin on the parent's pending ticket,
   deferred parent state, absent file before approval, and child/parent completion after
   the same ticket is approved.
4. The manual-run test proves the daemon dispatches a scheduled prompt into a real sidecar
   session and records completion. The due-run test also proves autonomous firing and
   missed-run accounting across an actual daemon crash/restart.
5. The memory test writes through the agent tool, restarts the daemon, and recalls the fact
   through the agent tool in a distinct session; secret writes are refused.
6. The Telegram fake receives streamed replies from a real daemon session, preserves the
   thread mapping across bot restart, and verifies all bot calls use the public API.
7. Chromium drives the daemon-served web app, approves a child write, resolves a secret-input
   ticket, and enters/exits takeover. The approval test also reloads and compares the whole
   rendered session screen reconstructed from durable events.
8. The fake push server verifies/decrypts authenticated Web Push ticket/completion payloads.
   A separate real Chromium service-worker test displays the notification and checks its
   deep link. Real mobile OS delivery remains a separate external acceptance requirement.
9. The existing sidecar test kills it during an active message, records that interrupted
   message as failed, restores the committed snapshot, and drains queued messages once.
   The older browser recovery test killed between turns and manually restarted the
   sidecar to load a later script. The new scenario kills Chromium while the original
   message is still running; the daemon automatically reconciles/relaunches it and that
   same message finishes through the same sidecar, without resubmission or manual restart.
10. The existing sync test already starts two real daemons against one credential-protected
    loopback fake WebDAV share. Settings, pins, vault entries, and selected memories reach
    the other instance, concurrent vault edits converge, and share bytes contain no
    plaintext canaries. No in-process substitute or duplicate sync test is needed.

## Additional acceptance evidence

PP-6: `web/src/state/session.test.ts` rebuilds the complete session view from events and
checks reconnect overlap. `web/src/hooks/useSessionView.test.tsx` compares live/reconnected
screen state with pure replay. The listed Chromium approval/reload scenario additionally
compares rendered text and the final event cursor before and after navigation reload.

B-5 has an optional real-daemon test:

```sh
cargo test -j 8 -p openside-e2e --test scenarios compositor_ipc_focus_is_unchanged_during_daemon_browser_work -- --exact --nocapture
```

It discovers `swaymsg` or `hyprctl` IPC, reads only the focused window identifier, and
samples it throughout real background browser navigation and interaction. An unavailable
IPC or absent active window prints a reason and skips; it is deliberately not part of the
ten mandatory scenarios above. This measures the daemon's current background/headless
browser mode, not an unimplemented headed-browser mode. On a non-graphical runner B-5 is
blocked-external, not PASS. User action: run the command from a graphical Wayland session
with a focused window, the compositor's IPC environment, Chromium, and the repository's
Python environment; leave the active window unchanged until the test completes.

## User-task lifecycle

`message.injected` starts the durable running state. Individual model-request
`turn.started`/`turn.ended` events remain unchanged, but `turn.ended` does not end the user
task. Running ends when all injected messages have completion outcomes, the session
settles, or it stops. Failed/aborted messages use the same completion boundary; a
nonterminal runtime warning does not finish the task. A deferred message pauses the
task and preserves the distinct public `waiting_human` status until its ticket is
resolved and the same message resumes.

The optional sleep inhibitor is held continuously through the task's browser/tool
calls and released on completion or while the task waits for a human ticket. The
web reducer follows the same durable boundaries, so replay during a tool call keeps
the screen running and leaves Stop/Thinking available. The multi-tool regression
in the table samples real daemon status and verifies one uninterrupted inhibitor
process across two slow loopback browser navigations.

## Recorded focused verification

- `tools/e2e-scenarios.sh`: all ten scenarios passed, with 15 listed real-process tests
  and no printed skips. Full command/test output and the PASS table are written to
  `target/e2e-scenarios.log`.
- `cargo test -j 8 -p openside-sessions`: 25 passed, including mid-task replay and
  deferred-message continuation.
- `pnpm -C web exec vitest run src/hooks/useSessionView.test.tsx src/state/session.test.ts`:
  12 passed, covering the replay reducer and reconnect hook.
- `python3 -m unittest discover -s tools/tests -p test_e2e_scenarios.py -v`: 7 passed,
  including exact selection, missing tests, ignored tests, and skip attribution.
- The optional B-5 command printed `SKIP: no reachable compositor IPC with an active window`.
  This is blocked-external evidence, not a successful compositor focus check.
