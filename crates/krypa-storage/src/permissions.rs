//! Per-origin permission decisions, keyed by (profile, origin, kind).
//!
//! `Origin` has no public constructor, so it is stored as `scheme://host:port` and rebuilt
//! through `NavigationUrl::parse(..).origin()`; the rebuilt origin must render back to the
//! stored text.

use krypa_engine_api::{PermissionDecision, PermissionKind, ProfileKind};
use krypa_navigation::{NavigationUrl, Origin, Scheme};
use rusqlite::OptionalExtension;

use crate::error::{corrupt, read_err, Result};
use crate::text::{clamp_limit, fetch_size, into_page, Page};
use crate::{Persisted, Storage, UnixMillis};

const TABLE: &str = "permissions";

/// One stored decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionRecord {
    /// The origin the decision applies to, and only to it.
    pub origin: Origin,
    /// What was asked for.
    pub kind: PermissionKind,
    /// The user's answer.
    pub decision: PermissionDecision,
    /// When it was given.
    pub decided_at: UnixMillis,
}

/// Position in the permission list (ordered by origin, then kind).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionCursor {
    origin: String,
    kind: String,
}

fn origin_key(origin: &Origin) -> String {
    let scheme = match origin.scheme() {
        Scheme::Https => "https",
        Scheme::Http => "http",
    };
    format!("{scheme}://{}:{}", origin.host(), origin.port())
}

fn origin_from_key(key: &str) -> Option<Origin> {
    let origin = NavigationUrl::parse(&format!("{key}/")).ok()?.origin();
    (origin_key(&origin) == key).then_some(origin)
}

fn kind_str(kind: PermissionKind) -> &'static str {
    match kind {
        PermissionKind::Camera => "camera",
        PermissionKind::Microphone => "microphone",
        PermissionKind::CameraAndMicrophone => "camera_and_microphone",
        PermissionKind::MotionAndOrientation => "motion_and_orientation",
        PermissionKind::Geolocation => "geolocation",
    }
}

fn parse_kind(s: &str) -> Option<PermissionKind> {
    Some(match s {
        "camera" => PermissionKind::Camera,
        "microphone" => PermissionKind::Microphone,
        "camera_and_microphone" => PermissionKind::CameraAndMicrophone,
        "motion_and_orientation" => PermissionKind::MotionAndOrientation,
        "geolocation" => PermissionKind::Geolocation,
        _ => return None,
    })
}

fn decision_str(d: PermissionDecision) -> &'static str {
    match d {
        PermissionDecision::Grant => "grant",
        PermissionDecision::Deny => "deny",
    }
}

fn parse_decision(s: &str) -> Option<PermissionDecision> {
    match s {
        "grant" => Some(PermissionDecision::Grant),
        "deny" => Some(PermissionDecision::Deny),
        _ => None,
    }
}

impl Storage {
    /// Stores (or replaces) a decision. A private session writes nothing and returns
    /// [`Persisted::SkippedPrivate`].
    ///
    /// # Errors
    /// [`crate::StorageError::UnknownProfile`] if a persistent profile does not exist.
    pub fn set_permission(
        &self,
        profile: &ProfileKind,
        origin: &Origin,
        kind: PermissionKind,
        decision: PermissionDecision,
        decided_at: UnixMillis,
    ) -> Result<Persisted> {
        let Some(profile_ref) = self.profile_row_for_write(profile)? else {
            return Ok(Persisted::SkippedPrivate);
        };
        self.conn.execute(
            "INSERT INTO permissions (profile_ref, origin, kind, decision, decided_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(profile_ref, origin, kind) DO UPDATE SET
                 decision = excluded.decision, decided_at = excluded.decided_at",
            rusqlite::params![
                profile_ref,
                origin_key(origin),
                kind_str(kind),
                decision_str(decision),
                decided_at
            ],
        )?;
        Ok(Persisted::Stored)
    }

    /// The stored decision for exactly this profile, origin and kind. Never answers for
    /// another origin (scheme, host and port all count), profile or kind; private
    /// sessions have none.
    ///
    /// # Errors
    /// [`crate::StorageError::CorruptRow`] if the stored decision is not valid.
    pub fn get_permission(
        &self,
        profile: &ProfileKind,
        origin: &Origin,
        kind: PermissionKind,
    ) -> Result<Option<PermissionDecision>> {
        let Some(profile_ref) = self.profile_row(profile)? else {
            return Ok(None);
        };
        let raw: Option<String> = self
            .conn
            .query_row(
                "SELECT decision FROM permissions
                 WHERE profile_ref = ?1 AND origin = ?2 AND kind = ?3",
                rusqlite::params![profile_ref, origin_key(origin), kind_str(kind)],
                |r| r.get(0),
            )
            .optional()
            .map_err(read_err(TABLE))?;
        raw.map(|d| parse_decision(&d).ok_or_else(|| corrupt(TABLE, None, "decision")))
            .transpose()
    }

    /// Forgets one decision. Returns whether it existed.
    ///
    /// # Errors
    /// [`crate::StorageError::Sqlite`] on database failure.
    pub fn clear_permission(
        &self,
        profile: &ProfileKind,
        origin: &Origin,
        kind: PermissionKind,
    ) -> Result<bool> {
        let Some(profile_ref) = self.profile_row(profile)? else {
            return Ok(false);
        };
        let n = self.conn.execute(
            "DELETE FROM permissions WHERE profile_ref = ?1 AND origin = ?2 AND kind = ?3",
            rusqlite::params![profile_ref, origin_key(origin), kind_str(kind)],
        )?;
        Ok(n > 0)
    }

    /// Lists the decisions of one profile ordered by origin, then kind.
    ///
    /// # Errors
    /// [`crate::StorageError::CorruptRow`] if a row on the page does not validate.
    pub fn list_permissions(
        &self,
        profile: &ProfileKind,
        limit: usize,
        after: Option<&PermissionCursor>,
    ) -> Result<Page<PermissionRecord, PermissionCursor>> {
        let Some(profile_ref) = self.profile_row(profile)? else {
            return Ok(Page {
                items: Vec::new(),
                next: None,
            });
        };
        let limit = clamp_limit(limit);
        let (origin, kind) = after.map_or((String::new(), String::new()), |c| {
            (c.origin.clone(), c.kind.clone())
        });
        let rows = self
            .conn
            .prepare(
                "SELECT origin, kind, decision, decided_at FROM permissions
                 WHERE profile_ref = ?1 AND (origin, kind) > (?2, ?3)
                 ORDER BY origin, kind LIMIT ?4",
            )?
            .query_map(
                rusqlite::params![profile_ref, origin, kind, fetch_size(limit)],
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
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
            .map(|(origin_text, kind_text, decision, decided_at)| {
                let record = PermissionRecord {
                    origin: origin_from_key(&origin_text)
                        .ok_or_else(|| corrupt(TABLE, None, "origin"))?,
                    kind: parse_kind(&kind_text).ok_or_else(|| corrupt(TABLE, None, "kind"))?,
                    decision: parse_decision(&decision)
                        .ok_or_else(|| corrupt(TABLE, None, "decision"))?,
                    decided_at,
                };
                Ok((record, origin_text, kind_text))
            })
            .collect::<Result<Vec<_>>>()?;
        let page = into_page(items, limit, |(_, o, k)| PermissionCursor {
            origin: o.clone(),
            kind: k.clone(),
        });
        Ok(Page {
            items: page.items.into_iter().map(|(r, _, _)| r).collect(),
            next: page.next,
        })
    }
}
