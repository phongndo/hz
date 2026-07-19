use std::{fmt, io, path::PathBuf};

pub type HzResult<T> = Result<T, HzError>;

#[derive(Debug)]
pub enum HzError {
    Io(io::Error),
    Json(serde_json::Error),
    UnknownWorkspace { target: String },
    WorkspaceNotInitialized(PathBuf),
    MarkerMismatch(PathBuf),
    MissingMarker(PathBuf),
    CowUnavailable(String),
    Usage(String),
}

impl fmt::Display for HzError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "{error}"),
            Self::Json(error) => write!(formatter, "{error}"),
            Self::UnknownWorkspace { target } => write!(formatter, "unknown workspace: {target}"),
            Self::WorkspaceNotInitialized(path) => {
                write!(
                    formatter,
                    "workspace is not initialized: {}",
                    path.display()
                )
            }
            Self::MarkerMismatch(path) => {
                write!(
                    formatter,
                    "workspace marker does not match the registry: {}",
                    path.display()
                )
            }
            Self::MissingMarker(path) => {
                write!(formatter, "workspace marker is missing: {}", path.display())
            }
            Self::CowUnavailable(message) => {
                write!(formatter, "copy-on-write cloning unavailable: {message}")
            }
            Self::Usage(message) => write!(formatter, "{message}"),
        }
    }
}

impl std::error::Error for HzError {}

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
