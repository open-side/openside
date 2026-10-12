//! `browser_snapshot` output (B-3).
//!
//! Every capture gets a `snapshot_id`. A diff is returned only when the caller names the base
//! it holds with `since`, and only if that base was delivered whole to the same conversation
//! (the session's parent agent or one subagent, separately) and is still in its context: when
//! the sidecar compacts that conversation the daemon bumps its context epoch, which retires
//! every earlier capture as a diff base. A diff can therefore never refer to text the caller
//! did not receive or no longer holds (a page, a scope, another conversation's capture, a
//! summarized-away tree). Paging (`from_line`, `from_char`) and scoping (`ref`) with
//! `snapshot_id` read the same immutable capture and are refused once its document is gone, so
//! pages never mix captures. Results are budgeted as serialized JSON below the host output
//! limit, so the host never cuts them.

use std::collections::HashMap;
use std::sync::LazyLock;

use openside_browser::{Browser, BrowserError, Snapshot, SnapshotPatch};
use openside_proto::SessionId;
use parking_lot::Mutex;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};

use super::super::ToolError;
use super::browser_error;

/// Serialized bytes of one result; below the host's 32 KiB limit with room for its envelope.
pub(crate) const RESULT_BUDGET: usize = 30 * 1024;
/// Captures kept for `since`/paging across all sessions; older ones expire.
const CAPTURES: usize = 64;
/// Scope hint names are labels for choosing a ref, not content; the ref returns them whole.
const HINT_NAME: usize = 80;

struct Capture {
    session: SessionId,
    /// `None` for the session's parent agent.
    subagent: Option<String>,
    /// The conversation's context epoch when the result was delivered.
    epoch: u64,
    tab: String,
    document: Option<String>,
    snapshot: Snapshot,
    /// The whole tree reached the caller in one result (or as a diff onto such a result).
    delivered_whole: bool,
    seq: u64,
}

#[derive(Default)]
struct Store {
    captures: HashMap<String, Capture>,
    /// Context epoch per conversation: `(session, subagent)`.
    epochs: HashMap<(String, Option<String>), u64>,
    seq: u64,
}

impl Store {
    fn epoch(&self, session: &SessionId, subagent: Option<&str>) -> u64 {
        let key = (session.as_str().to_owned(), subagent.map(str::to_owned));
        self.epochs.get(&key).copied().unwrap_or(0)
    }
}

/// The conversation `(session, subagent)` lost earlier messages from its model context, so
/// no capture it received before now can be assumed to still be there.
pub(crate) fn context_compacted(session: &SessionId, subagent: Option<&str>) {
    let key = (session.as_str().to_owned(), subagent.map(str::to_owned));
    *STORE.lock().epochs.entry(key).or_default() += 1;
}

/// Retire every capture of `tab` as a diff base and paging source.
pub(super) fn forget_tab(tab: &str) {
    STORE
        .lock()
        .captures
        .retain(|_, capture| capture.tab != tab);
}

// Not persisted: after a daemon restart every `since`/`snapshot_id` is unknown and the caller
// gets a full capture, which is always safe.
static STORE: LazyLock<Mutex<Store>> = LazyLock::new(Mutex::default);

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(super) struct SnapshotArgs {
    pub(super) tab_id: String,
    /// snapshot_id of the last complete snapshot of this tab you received; the result is then
    /// only the line diff against it (when the document is the same).
    since: Option<String>,
    /// Read from an existing capture (with from_line or ref) instead of taking a new one.
    snapshot_id: Option<String>,
    /// Only the subtree under this eN ref.
    #[serde(rename = "ref")]
    scope: Option<String>,
    /// 1-based line to start from, as given by a previous warning's next_from_line.
    from_line: Option<usize>,
    /// Characters of that line already received, as given by a previous warning's
    /// next_from_char; continues a single line that is longer than one result.
    from_char: Option<usize>,
}

fn stale() -> ToolError {
    ToolError(
        "snapshot_id is unknown, expired or from a document that is gone; take a new snapshot"
            .into(),
    )
}

/// Serve `from_line`/`ref` from an existing capture without taking a new one.
pub(super) async fn from_capture(
    session: &SessionId,
    browser: &Browser,
    args: &SnapshotArgs,
) -> Result<Option<Value>, ToolError> {
    let Some(id) = &args.snapshot_id else {
        return Ok(None);
    };
    let (snapshot, document) = {
        let store = STORE.lock();
        let capture = store
            .captures
            .get(id)
            .filter(|capture| &capture.session == session && capture.tab == args.tab_id)
            .ok_or_else(stale)?;
        (capture.snapshot.clone(), capture.document.clone())
    };
    let current = browser
        .document_id(&args.tab_id)
        .await
        .map_err(browser_error)?;
    if document.as_deref() != Some(current.as_str()) {
        return Err(stale());
    }
    view(id, &snapshot, args).map(|(value, _)| Some(value))
}

/// Result for a new capture: a diff against `since` when allowed, otherwise a (paged) view.
pub(super) fn output(
    session: &SessionId,
    subagent: Option<&str>,
    args: &SnapshotArgs,
    document: Option<&str>,
    snapshot: &Snapshot,
) -> Result<Value, ToolError> {
    let id = crate::persistence::random_id();
    let mut store = STORE.lock();
    let epoch = store.epoch(session, subagent);
    let base = args
        .since
        .as_ref()
        .filter(|_| args.scope.is_none() && args.from_line.is_none() && args.from_char.is_none())
        .and_then(|since| store.captures.get(since))
        .filter(|base| {
            &base.session == session
                && base.subagent.as_deref() == subagent
                && base.epoch == epoch
                && base.tab == args.tab_id
                && base.delivered_whole
                && base.document.is_some()
                && base.document.as_deref() == document
                && base.snapshot.document_root.is_some()
                && base.snapshot.document_root == snapshot.document_root
        });
    let mut result = None;
    if let (Some(base), Some(since)) = (base, &args.since) {
        let diff = SnapshotPatch::between(&base.snapshot.text, &snapshot.text).for_agent();
        let value = json!(format!("[snapshot_id {id} since {since}] {diff}"));
        if serialized(&value) <= RESULT_BUDGET {
            result = Some((value, true));
        }
    }
    let (value, whole) = match result {
        Some(result) => result,
        None => view(&id, snapshot, args)?,
    };
    store.seq += 1;
    let seq = store.seq;
    store.captures.insert(
        id,
        Capture {
            session: session.clone(),
            subagent: subagent.map(str::to_owned),
            epoch,
            tab: args.tab_id.clone(),
            document: document.map(str::to_owned),
            snapshot: snapshot.clone(),
            delivered_whole: whole,
            seq,
        },
    );
    if store.captures.len() > CAPTURES {
        if let Some(oldest) = store
            .captures
            .iter()
            .min_by_key(|(_, capture)| capture.seq)
            .map(|(id, _)| id.clone())
        {
            store.captures.remove(&oldest);
        }
    }
    Ok(value)
}

fn serialized(value: &Value) -> usize {
    serde_json::to_string(value).map_or(usize::MAX, |text| text.len())
}

/// A view of `snapshot` whose whole serialized result fits [`RESULT_BUDGET`]; the flag says
/// whether it holds the entire tree.
fn view(id: &str, snapshot: &Snapshot, args: &SnapshotArgs) -> Result<(Value, bool), ToolError> {
    let mut budget = RESULT_BUDGET - 1024;
    loop {
        let view = snapshot
            .view(
                args.scope.as_deref(),
                args.from_line.unwrap_or(1),
                args.from_char.unwrap_or(0),
                budget,
            )
            .map_err(|error| match error {
                BrowserError::StaleReference => {
                    ToolError("Unknown ref for this capture; take a fresh snapshot".into())
                }
                other => ToolError(other.to_string()),
            })?;
        let whole = view.omission.is_none() && args.scope.is_none();
        let value = match view.omission {
            // Complete results stay plain text; the first line carries the id for `since`.
            None => json!(format!("[snapshot_id {id}]\n{}", view.text)),
            Some(mut omission) => {
                for hint in &mut omission.scopes {
                    if hint.name.chars().count() > HINT_NAME {
                        hint.name = hint.name.chars().take(HINT_NAME).collect::<String>() + "…";
                    }
                }
                json!({"snapshot_id": id, "snapshot": view.text, "warning": omission})
            }
        };
        let size = serialized(&value);
        if size <= RESULT_BUDGET {
            return Ok((value, whole));
        }
        // Escaping in the warning or a shortened line pushed it over: shrink the text budget.
        budget = budget.saturating_sub(size - RESULT_BUDGET + 256);
        if budget < 1024 {
            return Err(ToolError("Snapshot result could not be bounded".into()));
        }
    }
}

#[cfg(test)]
mod tests {
    use openside_browser::SnapshotNode;

    use super::*;

    fn capture(names: &[&str]) -> Snapshot {
        Snapshot {
            nodes: names
                .iter()
                .enumerate()
                .map(|(index, name)| SnapshotNode {
                    reference: Some(format!("e{index}")),
                    role: "button".into(),
                    name: (*name).into(),
                    value: String::new(),
                    depth: 0,
                })
                .collect(),
            text: names
                .iter()
                .enumerate()
                .map(|(index, name)| format!("[e{index}] button {name:?}\n"))
                .collect(),
            raw_bytes: 0,
            document_root: Some(7),
        }
    }

    /// Deliver one capture to a conversation: `(snapshot_id, delivered as a diff)`.
    fn deliver(
        session: &SessionId,
        subagent: Option<&str>,
        since: Option<&str>,
        names: &[&str],
    ) -> (String, bool) {
        let args: SnapshotArgs =
            serde_json::from_value(json!({"tab_id": "tab", "since": since})).unwrap();
        let value = output(session, subagent, &args, Some("doc"), &capture(names))
            .map_err(|error| error.0)
            .unwrap();
        let text = value.as_str().expect("complete results are plain text");
        let header = text.strip_prefix("[snapshot_id ").unwrap();
        let header = header.split(']').next().unwrap();
        let (id, diff) = match header.split_once(" since ") {
            Some((id, _)) => (id, true),
            None => (header, false),
        };
        (id.to_owned(), diff)
    }

    #[test]
    fn compaction_retires_only_the_bases_of_the_conversation_that_lost_context() {
        let session = SessionId::new("epoch-unit-conversations");
        let (parent_full, _) = deliver(&session, None, None, &["a", "b"]);
        let (parent_diff, is_diff) = deliver(&session, None, Some(&parent_full), &["a", "b", "c"]);
        assert!(is_diff);
        let (child_full, _) = deliver(&session, Some("child"), None, &["a", "b", "c"]);
        // A base delivered to one conversation is not a base for another.
        assert!(!deliver(&session, None, Some(&child_full), &["a", "b", "c", "d"]).1);
        assert!(
            !deliver(
                &session,
                Some("child"),
                Some(&parent_diff),
                &["a", "b", "c", "d"]
            )
            .1
        );

        context_compacted(&session, None);
        // The parent kept the ids in its summary but lost the trees behind them: the full
        // tree comes back for the id it received whole and for a diff onto that base.
        assert!(!deliver(&session, None, Some(&parent_full), &["a", "b", "c"]).1);
        assert!(!deliver(&session, None, Some(&parent_diff), &["a", "b", "c"]).1);
        // The subagent's context was not compacted.
        assert!(
            deliver(
                &session,
                Some("child"),
                Some(&child_full),
                &["a", "b", "c", "d"]
            )
            .1
        );

        // A tree delivered after compaction is a valid base again.
        let (fresh, _) = deliver(&session, None, None, &["a", "b", "c"]);
        assert!(deliver(&session, None, Some(&fresh), &["a", "x", "c"]).1);

        context_compacted(&session, Some("child"));
        assert!(!deliver(&session, Some("child"), Some(&child_full), &["a"]).1);
        assert!(
            deliver(&session, None, Some(&fresh), &["a", "x", "c"]).1,
            "parent unaffected"
        );
    }

    #[test]
    fn a_diff_whose_base_chain_lost_its_middle_never_resolves_to_a_diff() {
        let session = SessionId::new("epoch-unit-chain");
        let (full, _) = deliver(&session, None, None, &["a"]);
        let (middle, middle_diff) = deliver(&session, None, Some(&full), &["a", "b"]);
        assert!(middle_diff);
        let (tip, tip_diff) = deliver(&session, None, Some(&middle), &["a", "b", "c"]);
        assert!(tip_diff);
        // Compaction kept `tip` (and its chain's ids) but summarized the trees away.
        context_compacted(&session, None);
        for base in [&full, &middle, &tip] {
            assert!(
                !deliver(&session, None, Some(base), &["a", "b", "c"]).1,
                "{base}"
            );
        }
    }
}
