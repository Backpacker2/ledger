use std::fmt;

/// What went wrong, in words fit for the UI: commands hand the message to the frontend as a plain string.
#[derive(Debug)]
pub enum Error {
    /// The request was understood but not acceptable (missing title, bad id, a guard like "column not empty").
    Invalid(String),
    NotFound(String),
    Db(rusqlite::Error),
    /// The vault (the folder of markdown files) could not be read or written.
    Io(String),
    /// Talking to GitHub, or reconciling with it, went wrong.
    Sync(String),
    /// GitHub moved on while a push was being prepared; the sync starts over from the new state.
    Rejected,
    /// Taking or delivering a screenshot went wrong.
    Screenshot(String),
}

pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    pub fn invalid(message: impl Into<String>) -> Self {
        Error::Invalid(message.into())
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Error::NotFound(message.into())
    }

    pub fn sync(message: impl Into<String>) -> Self {
        Error::Sync(message.into())
    }

    pub fn screenshot(message: impl std::fmt::Display) -> Self {
        Error::Screenshot(message.to_string())
    }

    /// An I/O failure with the path it happened on.
    pub fn io(path: &std::path::Path, e: std::io::Error) -> Self {
        Error::Io(format!("{}: {e}", path.display()))
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Invalid(message) | Error::NotFound(message) | Error::Sync(message) | Error::Screenshot(message) => f.write_str(message),
            Error::Db(e) => write!(f, "Database error: {e}"),
            Error::Io(message) => write!(f, "Vault error: {message}"),
            Error::Rejected => f.write_str("GitHub changed while syncing"),
        }
    }
}

impl std::error::Error for Error {}

impl From<rusqlite::Error> for Error {
    fn from(e: rusqlite::Error) -> Self {
        Error::Db(e)
    }
}

impl serde::Serialize for Error {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}
