//! Typed application errors. Tauri/HTTP boundaries still surface `String`
//! (French user-facing copy) via `From<AppError> for String`.

use std::sync::PoisonError;

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("{0}")]
    Message(String),

    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error("lock poisoned")]
    LockPoisoned,
}

pub type AppResult<T> = Result<T, AppError>;

impl AppError {
    pub fn msg(text: impl Into<String>) -> Self {
        Self::Message(text.into())
    }
}

impl From<AppError> for String {
    fn from(value: AppError) -> Self {
        value.to_string()
    }
}

impl From<String> for AppError {
    fn from(value: String) -> Self {
        Self::Message(value)
    }
}

impl From<&str> for AppError {
    fn from(value: &str) -> Self {
        Self::Message(value.to_string())
    }
}

impl<T> From<PoisonError<T>> for AppError {
    fn from(_: PoisonError<T>) -> Self {
        Self::LockPoisoned
    }
}
