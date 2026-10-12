//! The per-event policy check shared by pointer and keyboard input: authorize the node an event
//! will reach by its current accessibility role and name.

use super::{error::OpResult, locate::NodeRef, Driver};
use crate::{dom::ax_text, Target};

impl Driver {
    /// Authorize interacting with `node` as `action`, by its current accessibility role and
    /// name, which are read again on every call. Within one operation a target whose role and
    /// name are unchanged since it was authorized is not decided again, so one approval covers
    /// all events of one operation; a relabelled target is decided anew. The origins of the
    /// page and of the node's frame are checked every time.
    pub(super) async fn node_gate(
        &self,
        node: &NodeRef,
        action: &str,
        granted: &mut Vec<(NodeRef, String, String)>,
    ) -> OpResult<()> {
        let ax = self.gate_node(node).await?;
        let current = (node.clone(), ax_text(&ax["role"]), ax_text(&ax["name"]));
        if granted.contains(&current) {
            return self.gate_node_page(node, None).await.map(drop);
        }
        self.gate_node_page(
            node,
            Some(Target {
                action,
                role: &current.1,
                name: &current.2,
            }),
        )
        .await?;
        granted.push(current);
        Ok(())
    }
}
