use std::{fmt, io};

pub type HzResult<T> = Result<T, HzError>;

#[derive(Debug)]
pub enum HzError {
    Io(io::Error),
    Json(serde_json::Error),
    UnknownWorktree { target: String },
    Usage(String),
}

impl fmt::Display for HzError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "{error}"),
            Self::Json(error) => write!(formatter, "{error}"),
            Self::UnknownWorktree { target } => write!(formatter, "unknown worktree: {target}"),
            Self::Usage(message) => write!(formatter, "{message}"),
        }
    }
}

impl From<io::Error> for HzError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<serde_json::Error> for HzError {
    fn from(error: serde_json::Error) -> Self {
        Self::Json(error)
    }
}
