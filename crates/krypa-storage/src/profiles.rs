//! Profiles. The default profile is a fixed row created by migration 2; private sessions
//! have no row at all, which is what makes writing for them impossible.

use krypa_engine_api::{ProfileId, ProfileKind};
use rusqlite::OptionalExtension;

use crate::error::{corrupt, is_constraint, read_err, Result, StorageError};
use crate::text::{clamp_limit, fetch_size, into_page, sanitize_text, Page};
use crate::{Storage, UnixMillis};

const TABLE: &str = "profiles";
const MAX_ID_CHARS: usize = 128;
const MAX_NAME_CHARS: usize = 64;
const DEFAULT_PROFILE_ROW: i64 = 1;

/// A named persistent profile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Profile {
    /// Identifier shared with the engine (on iOS a UUID for the data store).
    pub id: ProfileId,
    /// Sanitized display name.
    pub name: String,
    /// Creation time.
    pub created_at: UnixMillis,
}

/// Position in the profile list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProfileCursor(i64);

fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.chars().count() <= MAX_ID_CHARS
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

impl Storage {
    /// Resolves a profile kind to its row. `Ok(None)` for private sessions and for a
    /// persistent profile that does not exist.
    pub(crate) fn profile_row(&self, kind: &ProfileKind) -> Result<Option<i64>> {
        match kind {
            ProfileKind::Private => Ok(None),
            ProfileKind::Default => Ok(Some(DEFAULT_PROFILE_ROW)),
            ProfileKind::Persistent(id) => Ok(self
                .conn
                .query_row(
                    "SELECT id FROM profiles WHERE profile_id = ?1",
                    [&id.0],
                    |r| r.get(0),
                )
                .optional()?),
        }
    }

    /// Like [`Self::profile_row`] for writes: a persistent profile that does not exist is
    /// an error. `Ok(None)` still means private.
    pub(crate) fn profile_row_for_write(&self, kind: &ProfileKind) -> Result<Option<i64>> {
        match (kind, self.profile_row(kind)?) {
            (ProfileKind::Private, _) => Ok(None),
            (_, Some(row)) => Ok(Some(row)),
            (_, None) => Err(StorageError::UnknownProfile),
        }
    }

    /// Creates a persistent profile.
    ///
    /// # Errors
    /// [`StorageError::InvalidInput`] for an id that is not 1-128 characters of
    /// `[A-Za-z0-9._-]` or a name that is empty after sanitizing;
    /// [`StorageError::ProfileExists`] for a duplicate id.
    pub fn create_profile(
        &self,
        id: &ProfileId,
        name: &str,
        created_at: UnixMillis,
    ) -> Result<Profile> {
        if !valid_id(&id.0) {
            return Err(StorageError::InvalidInput("invalid profile id"));
        }
        let name = sanitize_text(name, MAX_NAME_CHARS);
        if name.is_empty() {
            return Err(StorageError::InvalidInput("profile name is empty"));
        }
        match self.conn.execute(
            "INSERT INTO profiles (kind, profile_id, name, created_at)
             VALUES ('persistent', ?1, ?2, ?3)",
            rusqlite::params![id.0, name, created_at],
        ) {
            Ok(_) => Ok(Profile {
                id: id.clone(),
                name,
                created_at,
            }),
            Err(e) if is_constraint(&e) => Err(StorageError::ProfileExists),
            Err(e) => Err(e.into()),
        }
    }

    /// Lists persistent profiles in creation order. The default profile is implicit and
    /// not listed.
    ///
    /// # Errors
    /// [`StorageError::CorruptRow`] if a row does not validate.
    pub fn list_profiles(
        &self,
        limit: usize,
        after: Option<ProfileCursor>,
    ) -> Result<Page<Profile, ProfileCursor>> {
        let limit = clamp_limit(limit);
        let rows = self
            .conn
            .prepare(
                "SELECT id, profile_id, name, created_at FROM profiles
                 WHERE kind = 'persistent' AND id > ?1 ORDER BY id LIMIT ?2",
            )?
            .query_map(
                rusqlite::params![after.map_or(0, |c| c.0), fetch_size(limit)],
                |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, UnixMillis>(3)?,
                    ))
                },
            )?
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(read_err(TABLE))?;
        let items = rows
            .into_iter()
            .map(|(row, id, name, created_at)| {
                if !valid_id(&id) {
                    return Err(corrupt(TABLE, Some(row), "profile_id"));
                }
                if name.is_empty() || sanitize_text(&name, MAX_NAME_CHARS) != name {
                    return Err(corrupt(TABLE, Some(row), "name"));
                }
                Ok((
                    row,
                    Profile {
                        id: ProfileId(id),
                        name,
                        created_at,
                    },
                ))
            })
            .collect::<Result<Vec<_>>>()?;
        let page = into_page(items, limit, |(row, _)| ProfileCursor(*row));
        Ok(Page {
            items: page.items.into_iter().map(|(_, p)| p).collect(),
            next: page.next,
        })
    }

    /// Deletes a persistent profile with its history and permission decisions. Returns
    /// whether it existed. The default profile cannot be deleted.
    ///
    /// # Errors
    /// [`StorageError::Sqlite`] on database failure.
    pub fn delete_profile(&self, id: &ProfileId) -> Result<bool> {
        let n = self.conn.execute(
            "DELETE FROM profiles WHERE kind = 'persistent' AND profile_id = ?1",
            [&id.0],
        )?;
        self.scrub()?;
        Ok(n > 0)
    }
}
