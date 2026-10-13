//! Frames of an owned tab. Same-process frames share the tab's page session (their nodes have
//! page-session backend ids and their boxes are reported in top-viewport coordinates).
//! Out-of-process iframes (OOPIFs) are separate `iframe` targets with their own session, node
//! ids, backend ids and frame-local coordinates; `Page.getFrameTree` on the page session does
//! not list them. They are attached on demand with a browser-level `Target.attachToTarget`
//! (flattened, never waiting for a debugger) once their `parentFrameId` chain is proven to lead
//! to the owned tab, so a tab that is not being driven is never touched. A tab whose network is
//! tracked auto-attaches its OOPIFs instead (see [`adopt`]).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use serde_json::{json, Value};

use super::{requests, Browser, Inner, PAGE_COMMANDS};
use crate::snapshot::FrameTree;
use crate::BrowserError;

/// One frame of a tab, in frame-tree pre-order (the top frame first).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FrameInfo {
    /// CDP frame id; equals the target id for the top frame and for an OOPIF root.
    pub id: String,
    /// Parent frame id; None for the top frame.
    pub parent: Option<String>,
    /// The frame's `name` attribute value.
    pub name: String,
    /// Document URL.
    pub url: String,
    /// Security origin of the current document.
    pub origin: String,
    /// Loader id of the current document; changes on every navigation of the frame.
    pub loader: String,
    /// OOPIF target whose session hosts the frame; None for the tab's page session.
    pub host: Option<String>,
}

/// The node a top-viewport point hits, after descending into out-of-process iframes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FrameHit {
    /// OOPIF target whose session addresses `backend`; None for the page session.
    pub host: Option<String>,
    /// Frame id of the hit node's document.
    pub frame: String,
    /// Backend DOM node id within `host`.
    pub backend: u64,
}

fn collect(tree: &Value, parent: Option<&str>, host: Option<&str>, out: &mut Vec<FrameInfo>) {
    let frame = &tree["frame"];
    let Some(id) = frame["id"].as_str() else {
        return;
    };
    let text = |field: &str| frame[field].as_str().unwrap_or_default().to_owned();
    out.push(FrameInfo {
        id: id.to_owned(),
        parent: frame["parentId"].as_str().or(parent).map(str::to_owned),
        name: text("name"),
        url: text("url"),
        origin: text("securityOrigin"),
        loader: text("loaderId"),
        host: host.map(str::to_owned),
    });
    for child in tree["childFrames"].as_array().into_iter().flatten() {
        collect(child, Some(id), host, out);
    }
}

/// Reorder into tree pre-order from the top frame, keeping sibling order of discovery.
fn preorder(frames: Vec<FrameInfo>) -> Vec<FrameInfo> {
    let mut children: HashMap<Option<String>, Vec<FrameInfo>> = HashMap::new();
    for frame in frames {
        children
            .entry(frame.parent.clone())
            .or_default()
            .push(frame);
    }
    let mut ordered = Vec::new();
    let mut stack: Vec<FrameInfo> = children.remove(&None).unwrap_or_default();
    stack.reverse();
    while let Some(frame) = stack.pop() {
        if let Some(mut kids) = children.remove(&Some(frame.id.clone())) {
            kids.reverse();
            stack.extend(kids);
        }
        ordered.push(frame);
    }
    ordered
}

/// The default frame filter of snapshots: frames sharing the top frame's origin.
pub(super) fn same_origin(top: &FrameInfo, frame: &FrameInfo) -> bool {
    !frame.origin.is_empty() && frame.origin != "null" && frame.origin == top.origin
}

impl Browser {
    /// Every frame of an owned agent tab, including out-of-process iframes at any depth.
    pub async fn frames(&self, owner: &str, tab_id: &str) -> Result<Vec<FrameInfo>, BrowserError> {
        let session = self.owned_session(owner, tab_id).await?;
        self.frames_of(&session, tab_id).await
    }

    pub(super) async fn frames_of(
        &self,
        session: &str,
        tab_id: &str,
    ) -> Result<Vec<FrameInfo>, BrowserError> {
        let cdp = &self.inner.cdp;
        let tree = cdp
            .call(Some(session), "Page.getFrameTree", json!({}))
            .await?;
        let mut frames = Vec::new();
        collect(&tree["frameTree"], None, None, &mut frames);
        let mut targets = cdp.call(None, "Target.getTargets", json!({})).await?;
        let mut pending: Vec<(String, String)> = match targets["targetInfos"].take() {
            Value::Array(infos) => infos
                .into_iter()
                .filter(|info| info["type"] == "iframe")
                .filter_map(|info| {
                    Some((
                        info["targetId"].as_str()?.to_owned(),
                        info["parentFrameId"].as_str()?.to_owned(),
                    ))
                })
                .collect(),
            _ => Vec::new(),
        };
        // An OOPIF can sit in a frame of another OOPIF; attach in waves until no pending
        // target's parent frame belongs to this tab.
        loop {
            let known: HashSet<&str> = frames.iter().map(|frame| frame.id.as_str()).collect();
            let (ready, rest): (Vec<_>, Vec<_>) = pending
                .into_iter()
                .partition(|(_, parent)| known.contains(parent.as_str()));
            pending = rest;
            if ready.is_empty() {
                break;
            }
            for (target, parent) in ready {
                // A frame that went away between listing and attaching is simply not there.
                let Ok(child) = self.oopif_session(tab_id, &target).await else {
                    continue;
                };
                let Ok(tree) = cdp.call(Some(&child), "Page.getFrameTree", json!({})).await else {
                    continue;
                };
                collect(
                    &tree["frameTree"],
                    Some(&parent),
                    Some(&target),
                    &mut frames,
                );
            }
        }
        Ok(preorder(frames))
    }

    async fn oopif_session(&self, tab_id: &str, target: &str) -> Result<String, BrowserError> {
        if let Some((tab, session)) = self.inner.state.lock().await.oopif_sessions.get(target) {
            if tab == tab_id {
                return Ok(session.clone());
            }
            return Err(BrowserError::OwnershipDenied(target.to_owned()));
        }
        let attach = self.inner.attach_lock.lock().await;
        if let Some((_, session)) = self.inner.state.lock().await.oopif_sessions.get(target) {
            return Ok(session.clone());
        }
        let attached = self
            .inner
            .cdp
            .call(
                None,
                "Target.attachToTarget",
                json!({"targetId": target, "flatten": true}),
            )
            .await?;
        let session = attached["sessionId"]
            .as_str()
            .ok_or(BrowserError::InvalidResponse("iframe session"))?
            .to_owned();
        self.inner
            .state
            .lock()
            .await
            .oopif_sessions
            .insert(target.to_owned(), (tab_id.to_owned(), session.clone()));
        drop(attach);
        Ok(session)
    }

    /// The session that addresses nodes of `host` (None: the tab's page session).
    pub(super) async fn host_session(
        &self,
        owner: &str,
        tab_id: &str,
        host: Option<&str>,
    ) -> Result<String, BrowserError> {
        let page = self.owned_session(owner, tab_id).await?;
        let Some(host) = host else {
            return Ok(page);
        };
        let cached = self
            .inner
            .state
            .lock()
            .await
            .oopif_sessions
            .get(host)
            .cloned();
        match cached {
            Some((tab, session)) if tab == tab_id => Ok(session),
            Some(_) => Err(BrowserError::OwnershipDenied(host.to_owned())),
            // Detached since (a reload of the parent re-creates the target): rediscover.
            None => {
                self.frames_of(&page, tab_id).await?;
                match self.inner.state.lock().await.oopif_sessions.get(host) {
                    Some((tab, session)) if tab == tab_id => Ok(session.clone()),
                    _ => Err(BrowserError::StaleReference),
                }
            }
        }
    }

    /// Issue one [`PAGE_COMMANDS`] method on the session hosting `host`'s nodes.
    pub async fn host_command(
        &self,
        owner: &str,
        tab_id: &str,
        host: Option<&str>,
        method: &str,
        params: Value,
    ) -> Result<Value, BrowserError> {
        if !PAGE_COMMANDS.contains(&method) {
            return Err(BrowserError::InvalidAction("page command is not allowed"));
        }
        let session = self.host_session(owner, tab_id, host).await?;
        self.inner.cdp.call(Some(&session), method, params).await
    }

    /// Issue one [`PAGE_COMMANDS`] method on the session hosting frame `frame_id`: the page
    /// session for same-process frames, the OOPIF's own session otherwise.
    pub async fn frame_command(
        &self,
        owner: &str,
        tab_id: &str,
        frame_id: &str,
        method: &str,
        params: Value,
    ) -> Result<Value, BrowserError> {
        let frames = self.frames(owner, tab_id).await?;
        let frame = frames
            .iter()
            .find(|frame| frame.id == frame_id)
            .ok_or(BrowserError::StaleReference)?;
        self.host_command(owner, tab_id, frame.host.as_deref(), method, params)
            .await
    }

    /// Top-viewport position of the origin of `host`'s coordinate space: (0, 0) for the page
    /// session, else the content-box corner of the OOPIF's owner element, summed up its chain.
    pub async fn frame_offset(
        &self,
        owner: &str,
        tab_id: &str,
        host: Option<&str>,
    ) -> Result<(f64, f64), BrowserError> {
        let page = self.owned_session(owner, tab_id).await?;
        let frames = self.frames_of(&page, tab_id).await?;
        self.offset_in(owner, tab_id, &frames, host).await
    }

    async fn offset_in(
        &self,
        owner: &str,
        tab_id: &str,
        frames: &[FrameInfo],
        mut host: Option<&str>,
    ) -> Result<(f64, f64), BrowserError> {
        let (mut x, mut y) = (0.0, 0.0);
        while let Some(target) = host {
            let frame = frames
                .iter()
                .find(|frame| frame.id == target)
                .ok_or(BrowserError::StaleReference)?;
            let parent_host = frame
                .parent
                .as_deref()
                .and_then(|parent| frames.iter().find(|frame| frame.id == parent))
                .ok_or(BrowserError::StaleReference)?
                .host
                .as_deref();
            let owner_node = self
                .host_command(
                    owner,
                    tab_id,
                    parent_host,
                    "DOM.getFrameOwner",
                    json!({"frameId": target}),
                )
                .await?;
            let model = self
                .host_command(
                    owner,
                    tab_id,
                    parent_host,
                    "DOM.getBoxModel",
                    json!({"backendNodeId": owner_node["backendNodeId"]}),
                )
                .await?;
            let content = &model["model"]["content"];
            let (Some(left), Some(top)) = (content[0].as_f64(), content[1].as_f64()) else {
                return Err(BrowserError::InvalidResponse("frame owner box"));
            };
            x += left;
            y += top;
            host = parent_host;
        }
        Ok((x, y))
    }

    /// Hit-test a top-viewport point the way pointer input is routed: the page session's
    /// hit test descends into same-process frames by itself; an OOPIF owner element is
    /// followed into the OOPIF with the point translated into its coordinates. None when no
    /// node is at the point.
    pub async fn frame_hit(
        &self,
        owner: &str,
        tab_id: &str,
        x: f64,
        y: f64,
    ) -> Result<Option<FrameHit>, BrowserError> {
        let page = self.owned_session(owner, tab_id).await?;
        let Ok(mut hit) = self.node_at(&page, x, y, false).await else {
            return Ok(None);
        };
        let mut host: Option<String> = None;
        let mut frames: Option<Vec<FrameInfo>> = None;
        loop {
            let backend = hit["backendNodeId"]
                .as_u64()
                .ok_or(BrowserError::InvalidResponse("hit-test node"))?;
            let frame = hit["frameId"].as_str().unwrap_or_default().to_owned();
            let session = self.host_session(owner, tab_id, host.as_deref()).await?;
            let described = self
                .inner
                .cdp
                .call(
                    Some(&session),
                    "DOM.describeNode",
                    json!({"backendNodeId": backend}),
                )
                .await?;
            let child = described["node"]["frameId"].as_str().map(str::to_owned);
            if let Some(child) = child {
                if frames.is_none() {
                    frames = Some(self.frames_of(&page, tab_id).await?);
                }
                let all = frames.as_deref().unwrap_or_default();
                if all
                    .iter()
                    .any(|info| info.id == child && info.host.as_deref() == Some(&child))
                {
                    let (left, top) = self.offset_in(owner, tab_id, all, Some(&child)).await?;
                    let session = self.host_session(owner, tab_id, Some(&child)).await?;
                    let Ok(inner) = self.node_at(&session, x - left, y - top, false).await else {
                        return Ok(None);
                    };
                    hit = inner;
                    host = Some(child);
                    continue;
                }
            }
            return Ok(Some(FrameHit {
                host,
                frame,
                backend,
            }));
        }
    }

    /// Accessibility trees of the child frames `keep(top, frame)` accepts, parents first, for
    /// [`crate::snapshot::merge_frames`]. With `view`, only frames whose owner element in the
    /// top document is in view are taken.
    pub(super) async fn frame_trees(
        &self,
        session: &str,
        tab_id: &str,
        view: Option<&HashSet<u64>>,
        keep: &(dyn Fn(&FrameInfo, &FrameInfo) -> bool + Sync),
    ) -> Result<Vec<FrameTree>, BrowserError> {
        let frames = self.frames_of(session, tab_id).await?;
        let mut trees = Vec::new();
        let mut taken: HashSet<&str> = HashSet::new();
        let Some(top_info) = frames.first() else {
            return Ok(trees);
        };
        let top = Some(top_info.id.as_str());
        taken.insert(top_info.id.as_str());
        for frame in frames.iter().skip(1) {
            let Some(parent) = frame.parent.as_deref() else {
                continue;
            };
            if !taken.contains(parent) || !keep(top_info, frame) {
                continue;
            }
            let Some(parent_info) = frames.iter().find(|info| info.id == parent) else {
                continue;
            };
            let parent_session = match &parent_info.host {
                None => session.to_owned(),
                Some(host) => self.oopif_session(tab_id, host).await?,
            };
            let frame_session = match &frame.host {
                None => session.to_owned(),
                Some(host) => self.oopif_session(tab_id, host).await?,
            };
            let cdp = &self.inner.cdp;
            // A frame detached between listing and reading is left out, not an error.
            let Ok(owner) = cdp
                .call(
                    Some(&parent_session),
                    "DOM.getFrameOwner",
                    json!({"frameId": frame.id}),
                )
                .await
            else {
                continue;
            };
            let Some(owner) = owner["backendNodeId"].as_u64() else {
                continue;
            };
            if Some(parent) == top && view.is_some_and(|view| !view.contains(&owner)) {
                continue;
            }
            let Ok(ax) = cdp
                .call(
                    Some(&frame_session),
                    "Accessibility.getFullAXTree",
                    json!({"frameId": frame.id}),
                )
                .await
            else {
                continue;
            };
            taken.insert(frame.id.as_str());
            trees.push(FrameTree {
                frame: frame.id.clone(),
                host: frame.host.clone(),
                parent: (Some(parent) != top).then(|| parent.to_owned()),
                owner,
                ax,
            });
        }
        Ok(trees)
    }
}

/// A target auto-attached by a tracked tab's page session (or by one of its OOPIFs), held before
/// its first request: an iframe is recorded as the tab's OOPIF session, its requests are counted
/// toward the tab, and its own OOPIFs are auto-attached in turn before it is let run.
pub(super) async fn adopt(inner: &Arc<Inner>, event: &Value) {
    let params = &event["params"];
    let (Some(parent), Some(child)) = (event["sessionId"].as_str(), params["sessionId"].as_str())
    else {
        return;
    };
    let target = &params["targetInfo"];
    let iframe = target["type"] == "iframe";
    let worker = matches!(
        target["type"].as_str(),
        Some("worker" | "service_worker" | "shared_worker")
    );
    let tab = if let (true, Some(id)) = (iframe || worker, target["targetId"].as_str()) {
        let mut state = inner.state.lock().await;
        let tab = requests::session_tab(&state, parent).map(str::to_owned);
        if let Some(tab) = &tab {
            let sessions = if iframe {
                &mut state.oopif_sessions
            } else {
                &mut state.worker_sessions
            };
            sessions
                .entry(id.into())
                .or_insert_with(|| (tab.clone(), child.into()));
        }
        tab
    } else {
        None
    };
    let counted = (iframe || worker) && inner.network.adopt(child, parent);
    let waiting = params["waitingForDebugger"].as_bool().unwrap_or(false);
    if !counted && !waiting {
        return;
    }
    let inner = Arc::clone(inner);
    let child = child.to_owned();
    let target_id = target["targetId"].as_str().unwrap_or_default().to_owned();
    // The event task must keep draining events while these calls wait for their responses.
    tokio::spawn(async move {
        if let Some(tab) = tab {
            let restricted = {
                let state = inner.state.lock().await;
                state
                    .tabs
                    .get(&tab)
                    .and_then(|tab| tab.owner_session_id.as_deref())
                    .is_some_and(|owner| inner.network.owner_restricted(owner))
            };
            if requests::arm_session(&inner, &child, restricted, false)
                .await
                .is_err()
            {
                inner
                    .network
                    .document_denials
                    .lock()
                    .insert(tab, super::DocumentPermission::ConnectionDenied);
                inner
                    .cdp
                    .send(None, "Target.closeTarget", json!({"targetId":target_id}));
                return;
            }
            if counted && (iframe || restricted) {
                let _ = inner
                    .cdp
                    .call(Some(&child), "Network.enable", json!({}))
                    .await;
                if iframe {
                    let _ = inner
                        .cdp
                        .call(
                            Some(&child),
                            "Target.setAutoAttach",
                            json!({"autoAttach":true,"waitForDebuggerOnStart":true,"flatten":true}),
                        )
                        .await;
                }
            }
        }
        if waiting {
            let _ = inner
                .cdp
                .call(Some(&child), "Runtime.runIfWaitingForDebugger", json!({}))
                .await;
        }
    });
}
