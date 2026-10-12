#!/usr/bin/env bash
set -euo pipefail
: "${OPENSIDE_PATCHED_BROWSER:?set OPENSIDE_PATCHED_BROWSER to the patched chrome executable}"
[[ -x "$OPENSIDE_PATCHED_BROWSER" ]] || { printf 'FAIL: browser is not executable\n' >&2; exit 2; }
command -v setsid >/dev/null || { printf 'FAIL: setsid is required for isolated cleanup\n' >&2; exit 2; }
ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)"
work="$(mktemp -d /tmp/os-smoke.XXXXXX)"
runner=''
cleanup() {
    local status=$? file pid arg owned saved
    # A failure is kept for diagnosis; the temporary profiles themselves are not.
    if [[ "$status" -ne 0 && -f "$work/result.log" ]]; then
        saved="$ROOT/target/patched-smoke-failures/$(date +%Y%m%dT%H%M%S)-$$"
        mkdir -p -- "$saved"
        cp -- "$work/result.log" "$saved/"
        printf 'patched smoke failed; log kept in %s\n' "$saved" >&2
    fi
    if [[ -n "$runner" ]]; then kill -KILL -- "-$runner" 2>/dev/null || true; fi
    # Chromium detaches its process group; match only this invocation's temporary profiles.
    for file in /proc/[0-9]*/cmdline; do
        [[ -r "$file" ]] || continue
        pid="${file#/proc/}"; pid="${pid%/cmdline}"
        [[ "$pid" == "$$" ]] && continue
        owned=false
        while IFS= read -r -d '' arg; do
            [[ "$arg" == *"$work"* ]] && owned=true
        done <"$file" 2>/dev/null || true
        if [[ "$owned" == true ]]; then kill -KILL "$pid" 2>/dev/null || true; fi
    done
    rm -rf -- "$work"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
"$OPENSIDE_PATCHED_BROWSER" --version
export TMPDIR="$work"
set +e
setsid cargo test --manifest-path "$ROOT/Cargo.toml" -j 8 -p openside-browser --test patched -- --nocapture >"$work/result.log" 2>&1 &
runner=$!
wait "$runner"
result=$?
runner=''
cat "$work/result.log"
set -e
declare -A passed=()
while IFS= read -r line; do
    if [[ "$line" =~ ^test\ ([[:alnum:]_]+)\ \.\.\.\ ok$ ]]; then passed["${BASH_REMATCH[1]}"]=true; fi
done <"$work/result.log"
printf '\n%-8s %-6s %s\n' 'PATCH' 'RESULT' 'CHECK'
checks=(
    'P1|gateway_hello_publishes_private_discovery_and_rejects_bad_clients,gateway_runtime_dir_comes_from_the_switch_not_the_environment|Authenticated private gateway hello, runtime dir switch'
    'P2|agent_tabs_never_take_focus,window_activation_follows_the_requesting_actor|Focus unchanged, activation judged by actor (CDP, extension)'
    'P3/P4|ownership_ledger_reports_agent_tabs_in_their_group|Ownership ledger and Agent Tabs'
    'P5|agent_viewport_is_native_1440x900_and_hidden_tabs_keep_rendering,agent_viewport_stays_1440x900_after_resize_attempts,agent_viewport_has_one_owner_across_clients_and_tab_moves,agent_viewport_is_restored_when_another_client_leaves_without_a_resize,capture_slots_come_back_when_the_renderer_stops_answering,maximized_agent_window_is_emulated_at_1440x900|1440x900 DSF 1 with one owner, restored on client detach, bounded capture, maximized-window emulation'
    'P6|popup_dialog_and_download_events_reach_the_gateway|Popup/dialog/download events'
    'P7|automation_artifacts_are_absent|webdriver, native infobars, UA, Runtime leak'
    'P8|daemon_ui_new_tab_and_routines_open_existing_web_ui,daemon_ui_rejects_credentials_query_and_nonroot_switch_values|Daemon NTP/Ask entry, incognito and unsafe URL refusal'
    'P9|native_ux_defaults_are_enabled,browser_status_never_returns_sensitive_keywords_or_other_profile_state|Native defaults and profile-scoped safe status'
    'P10|native_autofill_checks_origin_scope_and_document_without_cdp_values,native_autofill_refuses_same_renderer_cross_origin_iframe_nodes,native_autofill_handles_stay_revoked_after_bfcache_restore|Native document/origin sink, iframe and BFCache refusal'
    'P11|daemon_ui_new_tab_and_routines_open_existing_web_ui|Existing web UI routines API list'
)
for row in "${checks[@]}"; do
    IFS='|' read -r patch test_names label <<<"$row"
    IFS=',' read -ra required <<<"$test_names"
    status=PASS
    for test_name in "${required[@]}"; do
        if [[ "${passed[$test_name]:-false}" != true ]]; then status=FAIL; result=1; fi
    done
    printf '%-8s %-6s %s\n' "$patch" "$status" "$label"
done
exit "$result"
