use std::collections::HashSet;

use crate::error::{log, ChannelError, Result};

/// Platform allow-lists. Every non-empty list must contain the update's matching ID;
/// at least one list must be non-empty, otherwise the adapter would serve everybody.
#[derive(Clone, Debug)]
pub struct AllowList {
    platform: &'static str,
    chats: HashSet<String>,
    users: HashSet<String>,
    workspaces: HashSet<String>,
}

impl AllowList {
    /// Build from configured IDs; all-empty is a configuration error.
    pub fn new(
        platform: &'static str,
        chats: &[String],
        users: &[String],
        workspaces: &[String],
    ) -> Result<Self> {
        if chats.is_empty() && users.is_empty() && workspaces.is_empty() {
            return Err(ChannelError::Config(format!(
                "{platform}: configure at least one allowed_* entry; an empty allow-list would serve nobody"
            )));
        }
        Ok(Self {
            platform,
            chats: chats.iter().cloned().collect(),
            users: users.iter().cloned().collect(),
            workspaces: workspaces.iter().cloned().collect(),
        })
    }

    /// Whether the update may be served. Denials are logged with IDs only.
    pub fn permits(&self, chat: &str, user: &str, workspace: Option<&str>) -> bool {
        let ok = (self.chats.is_empty() || self.chats.contains(chat))
            && (self.users.is_empty() || self.users.contains(user))
            && (self.workspaces.is_empty()
                || workspace.is_some_and(|w| self.workspaces.contains(w)));
        if !ok {
            log(format_args!(
                "{} update ignored: chat={chat} user={user} workspace={} not in an allow-list",
                self.platform,
                workspace.unwrap_or("-")
            ));
        }
        ok
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn empty_lists_are_rejected_and_every_configured_list_must_match() {
        assert!(AllowList::new("telegram", &[], &[], &[]).is_err());
        let list = AllowList::new("slack", &ids(&["C1"]), &ids(&["U1"]), &ids(&["T1"]))
            .unwrap_or_else(|e| panic!("{e}"));
        assert!(list.permits("C1", "U1", Some("T1")));
        assert!(!list.permits("C2", "U1", Some("T1")));
        assert!(!list.permits("C1", "U2", Some("T1")));
        assert!(!list.permits("C1", "U1", Some("T2")));
        assert!(!list.permits("C1", "U1", None));
    }

    #[test]
    fn a_single_list_is_enough() {
        let list =
            AllowList::new("telegram", &[], &ids(&["7"]), &[]).unwrap_or_else(|e| panic!("{e}"));
        assert!(list.permits("any-chat", "7", None));
        assert!(!list.permits("any-chat", "8", None));
    }
}
