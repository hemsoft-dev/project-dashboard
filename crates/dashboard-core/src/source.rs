//! The status-source boundary. Production uses pmd; there is no subprocess adapter.
use crate::status::StatusDocument;
use std::path::Path;

/// The retryable part of a read failure, also carried by partial Board/work results.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum TransportError {
    Timeout,
    Connect,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FetchError {
    MissingToken,
    Unauthorized,
    Timeout,
    Connect,
    Malformed,
    UnsupportedSchema,
    Identity,
    Untracked,
    Ambiguous,
    IdentityMismatch,
    NoStatus,
    Http(u16),
    Unavailable,
    Failed(String),
}
impl FetchError {
    /// Only transport failures are retried; HTTP and configuration errors stay visible.
    pub fn is_transient(&self) -> bool {
        self.transport().is_some()
    }

    pub fn transport(&self) -> Option<TransportError> {
        match self {
            Self::Timeout => Some(TransportError::Timeout),
            Self::Connect => Some(TransportError::Connect),
            _ => None,
        }
    }

    pub fn is_setup(&self) -> bool {
        matches!(self, Self::MissingToken | Self::Unauthorized)
    }
}
impl std::fmt::Display for FetchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::MissingToken => "pmd read token is missing, unreadable or invalid. Check pmd_token_path.",
            Self::Unauthorized => "pmd rejected the read token (HTTP 401).",
            Self::Connect => "Cannot connect to pmd.",
            Self::Timeout => "pmd request timed out.",
            Self::Malformed => "pmd returned malformed or incomplete data.",
            Self::UnsupportedSchema => "pmd returned an unsupported status schema version.",
            Self::Identity => "Repository identity unavailable: configure one GitHub origin owner/name.",
            Self::Untracked => "Repository is not tracked by pmd.",
            Self::Ambiguous => "Repository identity is ambiguous in pmd.",
            Self::IdentityMismatch => "pmd status repository does not match the configured checkout.",
            Self::NoStatus => "pmd has no_status for this project yet.",
            Self::Unavailable => "pmd is unavailable.",
            Self::Http(code) => return write!(f, "pmd returned HTTP {code}."),
            Self::Failed(message) => message,
        })
    }
}
impl std::error::Error for FetchError {}

pub trait StatusSource: Send + Sync {
    /// A manual refresh reads the API cache too. pmd v0.1 has no force-sync endpoint.
    fn fetch(&self, checkout: &Path, refresh: bool) -> Result<StatusDocument, FetchError>;
}
