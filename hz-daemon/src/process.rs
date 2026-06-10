use std::{
    process::{self, Command},
    time::{SystemTime, UNIX_EPOCH},
};

use hz_agent::AgentKind;
use hz_core::{HzError, HzResult};

pub(crate) fn generate_session_id() -> HzResult<String> {
    Ok(format!("{}-{}", process::id(), unix_nanos()?))
}

pub(crate) fn generate_agent_id(kind: AgentKind) -> HzResult<String> {
    Ok(format!("{}-{}", kind.command(), unix_nanos()?))
}

pub(crate) fn unix_seconds() -> HzResult<u64> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| HzError::Usage(format!("system clock is before unix epoch: {error}")))?
        .as_secs())
}

fn unix_nanos() -> HzResult<u128> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| HzError::Usage(format!("system clock is before unix epoch: {error}")))?
        .as_nanos())
}

pub(crate) fn terminate_process_group(pid: u32) -> HzResult<()> {
    let status = Command::new("kill")
        .arg("-TERM")
        .arg(format!("-{}", pid))
        .status()?;
    if status.success() {
        return Ok(());
    }

    let status = Command::new("kill")
        .arg("-TERM")
        .arg(pid.to_string())
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(HzError::Usage(format!(
            "failed to stop agent process {pid}"
        )))
    }
}

pub(crate) fn process_is_running(pid: u32) -> bool {
    Command::new("kill")
        .arg("-0")
        .arg(pid.to_string())
        .status()
        .is_ok_and(|status| status.success())
}
