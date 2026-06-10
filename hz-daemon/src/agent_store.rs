use std::{fs, io};

use hz_agent::AgentSession;
use hz_core::HzResult;
use serde::{Deserialize, Serialize};

use crate::paths::agents_file;

#[derive(Debug, Default, Serialize, Deserialize)]
pub(crate) struct AgentStore {
    pub(crate) agents: std::collections::BTreeMap<String, AgentSession>,
}

impl AgentStore {
    pub(crate) fn load() -> HzResult<Self> {
        let path = agents_file()?;
        match fs::read(&path) {
            Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(error.into()),
        }
    }

    pub(crate) fn save(&self) -> HzResult<()> {
        let path = agents_file()?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, serde_json::to_vec_pretty(self)?)?;
        Ok(())
    }
}
