use std::fmt;

use crate::crypto::CryptoError;

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
    /// A file on GitHub could not be decrypted (or was not encrypted). The sync stops before it changes anything here.
    Crypto { path: String, cause: CryptoError },
    /// Looking for, downloading or installing an update of Ledger went wrong.
    Update(String),
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

    pub fn update(message: impl Into<String>) -> Self {
        Error::Update(message.into())
    }

    /// An I/O failure with the path it happened on.
    pub fn io(path: &std::path::Path, e: std::io::Error) -> Self {
        Error::Io(format!("{}: {e}", path.display()))
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Invalid(message) | Error::NotFound(message) | Error::Sync(message) | Error::Screenshot(message) | Error::Update(message) => f.write_str(message),
            Error::Db(e) => write!(f, "Database error: {e}"),
            Error::Io(message) => write!(f, "Vault error: {message}"),
            Error::Rejected => f.write_str("GitHub changed while syncing"),
            Error::Crypto { path, cause } => {
                write!(f, "{path} on GitHub {cause}. Syncing stopped and nothing on this machine was changed.")?;
                if matches!(cause, CryptoError::NotEncrypted) {
                    f.write_str(
                        " A branch that holds unencrypted files can't become an encrypted one, because their history would stay on GitHub. Point sync at a new repository, or at a branch that doesn't exist yet, under Sync to GitHub.",
                    )?;
                }
                Ok(())
            }
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
