use thiserror::Error;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Error)]
pub enum Error {
    #[error("{0}")]
    Invalid(String),
    #[error("conflict: {0}")]
    Conflict(String),
    #[error("integrity failure: {0}")]
    Integrity(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("dependency requirements are not satisfied; inspect knot graph --json")]
    UnsatisfiedDependencies(Vec<crate::DependencyEdge>),
    #[error("repository is busy")]
    Busy,
    #[error("unfinished transaction; run knot recover --continue or knot recover --abort")]
    Pending,
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("invalid object encoding: {0}")]
    Encoding(#[from] serde_json::Error),
}

impl Error {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Invalid(_) => "invalid_input",
            Self::Conflict(_) => "conflict",
            Self::UnsatisfiedDependencies(_) => "dependency_conflict",
            Self::Integrity(_) | Self::Encoding(_) => "integrity_failure",
            Self::NotFound(_) => "not_found",
            Self::Busy => "repository_busy",
            Self::Pending => "recovery_required",
            Self::Io(_) => "io_error",
        }
    }

    pub fn details(&self) -> serde_json::Value {
        match self {
            Self::UnsatisfiedDependencies(conflicts) => serde_json::json!({"conflicts":conflicts}),
            _ => serde_json::json!({}),
        }
    }
    pub fn exit_code(&self) -> u8 {
        match self {
            Self::Invalid(_) => 2,
            Self::Conflict(_) | Self::Pending | Self::UnsatisfiedDependencies(_) => 3,
            Self::Integrity(_) | Self::Encoding(_) => 4,
            Self::Busy => 5,
            Self::Io(_) | Self::NotFound(_) => 1,
        }
    }
}
