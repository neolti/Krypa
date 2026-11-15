//! SQLite persistence for Krypa's own metadata: installed apps, favorites, settings,
//! navigation history, profiles and per-origin permission decisions.
//!
//! Website data (cookies, caches, site storage) belongs to the web engine and is never
//! stored or touched here.
//!
//! Rules this crate enforces:
//! * Anything that would persist browsing activity takes a [`ProfileKind`]; for
//!   [`ProfileKind::Private`] it writes nothing and reports [`Persisted::SkippedPrivate`].
//! * Everything read back is untrusted. Rows are re-validated through the validating
//!   constructors of the domain crates; a bad row is a [`StorageError::CorruptRow`], never
//!   a panic and never silently skipped.
//! * URLs, titles and origins are never logged and never appear in error values.
//! * Statements are parameterized; the SQL text is constant.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]
#![deny(missing_docs)]

mod apps;
mod error;
mod history;
mod migrations;
mod permissions;
mod profiles;
mod settings;
mod text;

#[cfg(test)]
mod tests;

use std::path::Path;
use std::time::Duration;

use rusqlite::Connection;

pub use apps::{AppCursor, AppSummary, FavoriteCursor, InstalledAppsPage};
pub use error::{Result, StorageError};
pub use history::{HistoryCursor, HistoryEntry, TimeRange};
pub use permissions::{PermissionCursor, PermissionRecord};
pub use profiles::{Profile, ProfileCursor};
pub use settings::{SettingValue, MAX_SETTING_TEXT_BYTES};
pub use text::{Page, MAX_PAGE};

/// Milliseconds since the Unix epoch. Callers supply times so behaviour is deterministic.
pub type UnixMillis = i64;

/// Whether a write that records browsing activity reached the database.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Persisted {
    /// The data was written.
    Stored,
    /// The profile is private, so nothing was written.
    SkippedPrivate,
}

/// Handle to the Krypa metadata database.
#[derive(Debug)]
pub struct Storage {
    pub(crate) conn: Connection,
}

impl Storage {
    /// Opens (creating if needed) the database at `path`, enables foreign keys and WAL,
    /// and applies pending migrations.
    ///
    /// # Errors
    /// [`StorageError::SchemaTooNew`] if the file was written by a newer version (it is
    /// left untouched), [`StorageError::Migration`] if a migration fails (rolled back),
    /// [`StorageError::WalUnavailable`], or [`StorageError::Sqlite`].
    pub fn open(path: &Path) -> Result<Self> {
        let mut conn = Connection::open(path)?;
        conn.busy_timeout(Duration::from_secs(5))?;
        let found = migrations::user_version(&conn)?;
        if found > migrations::latest() {
            return Err(StorageError::SchemaTooNew {
                found,
                supported: migrations::latest(),
            });
        }
        conn.pragma_update(None, "foreign_keys", true)?;
        let mode: String =
            conn.pragma_update_and_check(None, "journal_mode", "WAL", |r| r.get(0))?;
        if !mode.eq_ignore_ascii_case("wal") {
            return Err(StorageError::WalUnavailable);
        }
        // Deleted history must not linger in free pages.
        conn.pragma_update(None, "secure_delete", true)?;
        conn.pragma_update(None, "cell_size_check", true)?;
        migrations::migrate_to(&mut conn, migrations::latest())?;
        Ok(Self { conn })
    }

    /// The schema version of the open database.
    ///
    /// # Errors
    /// [`StorageError::Sqlite`] if the pragma cannot be read.
    pub fn schema_version(&self) -> Result<u32> {
        migrations::user_version(&self.conn)
    }

    /// Flushes the write-ahead log into the main file and truncates it, so deleted rows
    /// do not survive in the log.
    pub(crate) fn scrub(&self) -> Result<()> {
        self.conn
            .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))?;
        Ok(())
    }
}
