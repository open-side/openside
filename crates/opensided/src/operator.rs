use std::{collections::BTreeMap, path::PathBuf};

pub(crate) struct State {
    pub(crate) lifecycle: crate::lifecycle::Lifecycle,
    pub(crate) model: crate::memory_model::Job,
    pub(crate) agents: parking_lot::RwLock<BTreeMap<String, PathBuf>>,
    pub(crate) doctor: tokio::sync::Mutex<()>,
}

impl State {
    pub(crate) fn new(config: &openside_config::Config) -> Self {
        Self {
            lifecycle: crate::lifecycle::Lifecycle::new(),
            model: Default::default(),
            agents: parking_lot::RwLock::new(config.skills.agents.clone()),
            doctor: tokio::sync::Mutex::new(()),
        }
    }
}
