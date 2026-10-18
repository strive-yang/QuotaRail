use serde::Serialize;
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    InvalidArguments,
    CliNotFound,
    UnsupportedLauncher,
    UnsupportedVersion,
    SpawnFailed,
    ProcessIsolation,
    Timeout,
    Cancelled,
    ChildExited,
    Protocol,
    MessageTooLarge,
    SignedOut,
    UnsupportedAuth,
    AuthenticationRejected,
    AccessDenied,
    RequestFailed,
    Io,
}

/// Only static, reviewed diagnostics may cross the output boundary.
#[derive(Debug, Clone, Serialize)]
pub struct ProbeError {
    pub kind: ErrorKind,
    pub message: &'static str,
}

impl ProbeError {
    pub const fn new(kind: ErrorKind, message: &'static str) -> Self {
        Self { kind, message }
    }

    pub fn exit_code(&self) -> i32 {
        match self.kind {
            ErrorKind::InvalidArguments => 2,
            ErrorKind::SignedOut
            | ErrorKind::UnsupportedAuth
            | ErrorKind::AuthenticationRejected => 3,
            ErrorKind::Timeout => 4,
            ErrorKind::Cancelled => 130,
            _ => 1,
        }
    }
}

impl fmt::Display for ProbeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}: {}", self.kind, self.message)
    }
}

impl std::error::Error for ProbeError {}

pub type Result<T> = std::result::Result<T, ProbeError>;
