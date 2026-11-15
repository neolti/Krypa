use thiserror::Error;

/// Why a storage operation failed.
///
/// No variant carries a URL, title or origin, so `Display` and `Debug` are safe to log.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum StorageError {
    /// SQLite reported an error.
    #[error("database error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    /// The file was written by a newer version of Krypa. It is never downgraded.
    #[error("database schema version {found} is newer than the supported version {supported}")]
    SchemaTooNew {
        /// `user_version` found in the file.
        found: u32,
        /// Highest version this build knows.
        supported: u32,
    },
    /// A migration failed and was rolled back.
    #[error("migration to schema version {version} failed: {source}")]
    Migration {
        /// Version the migration was moving to.
        version: u32,
        /// Underlying error.
        source: rusqlite::Error,
    },
    /// The file system does not support WAL mode.
    #[error("write-ahead logging could not be enabled")]
    WalUnavailable,
    /// A stored row failed validation or could not be decoded; the file is corrupt or was
    /// tampered with. `row` is the SQLite rowid when the table has one.
    #[error("corrupt row in `{table}` (row {row:?}): invalid {field}")]
    CorruptRow {
        /// Table the row came from.
        table: &'static str,
        /// Row identifier, when known.
        row: Option<i64>,
        /// Column or value that failed.
        field: &'static str,
    },
    /// The caller passed a value that may not be stored.
    #[error("invalid input: {0}")]
    InvalidInput(&'static str),
    /// The profile does not exist.
    #[error("unknown profile")]
    UnknownProfile,
    /// The app does not exist.
    #[error("unknown app")]
    UnknownApp,
    /// A profile with this id already exists.
    #[error("profile already exists")]
    ProfileExists,
}

/// Result alias for this crate.
pub type Result<T> = std::result::Result<T, StorageError>;

pub(crate) fn corrupt(table: &'static str, row: Option<i64>, field: &'static str) -> StorageError {
    StorageError::CorruptRow { table, row, field }
}

/// Maps a read error: a value of the wrong type is corruption, anything else is passed on.
pub(crate) fn read_err(table: &'static str) -> impl Fn(rusqlite::Error) -> StorageError {
    move |e| match e {
        rusqlite::Error::InvalidColumnType(..)
        | rusqlite::Error::FromSqlConversionFailure(..)
        | rusqlite::Error::IntegralValueOutOfRange(..)
        | rusqlite::Error::Utf8Error(..) => corrupt(table, None, "column type"),
        other => StorageError::Sqlite(other),
    }
}

/// True for a constraint violation (foreign key, unique, check).
pub(crate) fn is_constraint(e: &rusqlite::Error) -> bool {
    matches!(
        e,
        rusqlite::Error::SqliteFailure(f, _) if f.code == rusqlite::ErrorCode::ConstraintViolation
    )
}
