use crate::ReasonCode;
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppErrorKind {
    Unsupported,
    AuthenticationRequired,
    Discovery,
    Protocol,
    InvalidFixture,
    Io,
}

impl AppErrorKind {
    pub const fn exit_code(self) -> u8 {
        match self {
            Self::Unsupported => 2,
            Self::AuthenticationRequired => 3,
            Self::Discovery => 4,
            Self::Protocol | Self::InvalidFixture | Self::Io => 5,
        }
    }
}

#[derive(Debug, Error)]
#[error("{safe_message}")]
pub struct AppError {
    pub kind: AppErrorKind,
    pub reason_code: ReasonCode,
    pub safe_message: String,
}

impl AppError {
    pub fn new<R, M>(kind: AppErrorKind, reason_code: R, safe_message: M) -> Self
    where
        R: TryInto<ReasonCode>,
        R::Error: std::fmt::Debug,
        M: Into<String>,
    {
        let reason_code = reason_code
            .try_into()
            .expect("application reason codes must be valid ASCII identifiers");
        Self {
            kind,
            reason_code,
            safe_message: safe_message.into(),
        }
    }

    pub const fn exit_code(&self) -> u8 {
        self.kind.exit_code()
    }
}
