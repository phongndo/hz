use std::time::Duration;

use hz_core::{HzError, HzResult};
use serde::{Deserialize, Serialize};

pub(crate) const SOCKET_FILE: &str = "daemon-v2.sock";
pub(crate) const PID_FILE: &str = "daemon-v2.pid";
pub(crate) const AGENTS_FILE: &str = "agents.json";
pub(crate) const CONNECT_TIMEOUT: Duration = Duration::from_millis(250);
pub(crate) const START_TIMEOUT: Duration = Duration::from_secs(2);
pub(crate) const START_POLL: Duration = Duration::from_millis(20);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct SendAgentInput {
    pub(crate) id: String,
    pub(crate) input: String,
}

pub(crate) enum DaemonReply {
    Ok(String),
    Error(String),
    Stop(String),
}

impl DaemonReply {
    pub(crate) fn ok(message: String) -> Self {
        Self::Ok(message)
    }

    pub(crate) fn error(message: impl Into<String>) -> Self {
        Self::Error(message.into())
    }

    pub(crate) fn line(&self) -> String {
        match self {
            Self::Ok(message) => format!("OK {message}"),
            Self::Error(message) => format!("ERR {message}"),
            Self::Stop(message) => format!("OK {message}"),
        }
    }
}

pub(crate) fn parse_response(response: &str) -> HzResult<String> {
    if response == "OK" {
        return Ok(String::new());
    }
    if let Some(payload) = response.strip_prefix("OK ") {
        return Ok(payload.to_owned());
    }
    if let Some(message) = response.strip_prefix("ERR ") {
        return Err(HzError::Usage(message.to_owned()));
    }

    Err(HzError::Usage(format!(
        "invalid hz daemon response: {response}"
    )))
}

pub(crate) fn json_payload<'a>(line: &'a str, command: &str) -> Result<&'a str, String> {
    line.strip_prefix(command)
        .and_then(|rest| rest.strip_prefix(' '))
        .filter(|payload| !payload.is_empty())
        .ok_or_else(|| format!("missing {command} payload"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn daemon_response_parser_handles_ok_and_error() {
        assert_eq!(
            parse_response("OK attached session").unwrap(),
            "attached session"
        );
        assert_eq!(parse_response("OK").unwrap(), "");
        assert!(
            matches!(parse_response("ERR bad"), Err(HzError::Usage(message)) if message == "bad")
        );
    }
}
