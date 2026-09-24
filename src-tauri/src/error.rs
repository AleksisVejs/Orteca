//! One error type crossing the Tauri boundary. The frontend switches on `kind`
//! and shows `message`; it never parses prose.

use serde::Serialize;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppError {
    pub kind: ErrorKind,
    pub message: String,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ErrorKind {
    NotFound,
    NotAGitRepo,
    /// The provider CLI is not on PATH. A state the UI explains, not a crash.
    CliMissing,
    /// The project has not been consented to. A run loads that repo's hooks.
    NotTrusted,
    /// Not a failure: the request needs one answer before a run is worth it.
    /// The message is the question.
    Clarify,
    Invalid,
    Io,
    Db,
}

impl AppError {
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        AppError {
            kind,
            message: message.into(),
        }
    }
}

impl From<std::io::Error> for AppError {
    fn from(e: std::io::Error) -> Self {
        AppError::new(ErrorKind::Io, e.to_string())
    }
}

impl From<rusqlite::Error> for AppError {
    fn from(e: rusqlite::Error) -> Self {
        AppError::new(ErrorKind::Db, e.to_string())
    }
}

pub type Result<T> = std::result::Result<T, AppError>;
