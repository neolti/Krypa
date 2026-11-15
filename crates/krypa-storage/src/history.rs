//! Navigation history. Only non-private profiles can be written.

use krypa_engine_api::ProfileKind;
use krypa_navigation::NavigationUrl;

use crate::error::{corrupt, read_err, Result, StorageError};
use crate::text::{clamp_limit, fetch_size, into_page, sanitize_text, Page};
use crate::{Persisted, Storage, UnixMillis};

const TABLE: &str = "history";
const MAX_TITLE_CHARS: usize = 256;

/// One visit, as a history list needs it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryEntry {
    /// Row id, usable as a stable key.
    pub id: i64,
    /// The visited page, re-validated on load.
    pub url: NavigationUrl,
    /// Sanitized page title, if the page had one.
    pub title: Option<String>,
    /// Visit time.
    pub visited_at: UnixMillis,
}

/// Position in the history list (newest first).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HistoryCursor {
    visited_at: UnixMillis,
    id: i64,
}

/// An inclusive time range, `start <= visited_at <= end`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimeRange {
    /// First instant included.
    pub start: UnixMillis,
    /// Last instant included.
    pub end: UnixMillis,
}

impl TimeRange {
    /// Every time.
    #[must_use]
    pub fn all() -> Self {
        Self {
            start: i64::MIN,
            end: i64::MAX,
        }
    }
}

impl Storage {
    /// Records a visit. A private session writes nothing and returns
    /// [`Persisted::SkippedPrivate`].
    ///
    /// # Errors
    /// [`StorageError::UnknownProfile`] if a persistent profile does not exist.
    pub fn record_visit(
        &self,
        profile: &ProfileKind,
        url: &NavigationUrl,
        title: Option<&str>,
        visited_at: UnixMillis,
    ) -> Result<Persisted> {
        let Some(profile_ref) = self.profile_row_for_write(profile)? else {
            return Ok(Persisted::SkippedPrivate);
        };
        let title = title
            .map(|t| sanitize_text(t, MAX_TITLE_CHARS))
            .filter(|t| !t.is_empty());
        self.conn.execute(
            "INSERT INTO history (profile_ref, url, title, visited_at) VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![profile_ref, url.as_str(), title, visited_at],
        )?;
        Ok(Persisted::Stored)
    }

    /// Lists history newest first. Private sessions and unknown profiles have none.
    ///
    /// # Errors
    /// [`StorageError::CorruptRow`] if a row on the page does not validate.
    pub fn list_history(
        &self,
        profile: &ProfileKind,
        limit: usize,
        after: Option<&HistoryCursor>,
    ) -> Result<Page<HistoryEntry, HistoryCursor>> {
        let Some(profile_ref) = self.profile_row(profile)? else {
            return Ok(Page {
                items: Vec::new(),
                next: None,
            });
        };
        let limit = clamp_limit(limit);
        let (at, id) = after.map_or((i64::MAX, i64::MAX), |c| (c.visited_at, c.id));
        // `(visited_at, id) < (at, id)`; the first page starts above every possible row.
        let rows = self
            .conn
            .prepare(
                "SELECT id, url, title, visited_at FROM history
                 WHERE profile_ref = ?1 AND (visited_at, id) < (?2, ?3)
                 ORDER BY visited_at DESC, id DESC LIMIT ?4",
            )?
            .query_map(
                rusqlite::params![profile_ref, at, id, fetch_size(limit)],
                |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, Option<String>>(2)?,
                        r.get::<_, UnixMillis>(3)?,
                    ))
                },
            )?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(read_err(TABLE))?;
        let items = rows
            .into_iter()
            .map(|(id, url, title, visited_at)| {
                let url =
                    NavigationUrl::parse(&url).map_err(|_| corrupt(TABLE, Some(id), "url"))?;
                if let Some(t) = &title {
                    if t.is_empty() || sanitize_text(t, MAX_TITLE_CHARS) != *t {
                        return Err(corrupt(TABLE, Some(id), "title"));
                    }
                }
                Ok(HistoryEntry {
                    id,
                    url,
                    title,
                    visited_at,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(into_page(items, limit, |e| HistoryCursor {
            visited_at: e.visited_at,
            id: e.id,
        }))
    }

    /// Deletes the visits of `profile` inside `range` and flushes the write-ahead log so
    /// they do not survive on disk. Returns the number of rows deleted. Private sessions
    /// have nothing to delete.
    ///
    /// # Errors
    /// [`StorageError::InvalidInput`] if `range.start > range.end`.
    pub fn clear_history(&self, profile: &ProfileKind, range: TimeRange) -> Result<usize> {
        if range.start > range.end {
            return Err(StorageError::InvalidInput("time range is reversed"));
        }
        let Some(profile_ref) = self.profile_row(profile)? else {
            return Ok(0);
        };
        let n = self.conn.execute(
            "DELETE FROM history WHERE profile_ref = ?1 AND visited_at BETWEEN ?2 AND ?3",
            rusqlite::params![profile_ref, range.start, range.end],
        )?;
        self.scrub()?;
        Ok(n)
    }
}
