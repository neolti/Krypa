use rusqlite::{Connection, TransactionBehavior};

use crate::error::{Result, StorageError};

/// Forward-only migrations. Index `i` upgrades `user_version` `i` to `i + 1`. Never edit
/// a released entry; append a new one.
pub(crate) const MIGRATIONS: [&str; 2] = [
    // 1: installed apps, favorites, settings.
    "
    CREATE TABLE installed_apps (
        id               TEXT PRIMARY KEY NOT NULL,
        name             TEXT NOT NULL,
        start_url        TEXT NOT NULL,
        scope            TEXT NOT NULL,
        display          TEXT NOT NULL CHECK (display IN ('standalone', 'minimal-ui', 'browser')),
        theme_color      TEXT,
        background_color TEXT,
        installed_at     INTEGER NOT NULL
    ) STRICT;
    CREATE INDEX idx_installed_apps_name ON installed_apps (name, id);

    CREATE TABLE app_icons (
        app_id   TEXT NOT NULL REFERENCES installed_apps (id) ON DELETE CASCADE,
        position INTEGER NOT NULL,
        src      TEXT NOT NULL,
        sizes    TEXT NOT NULL,
        purposes TEXT NOT NULL,
        PRIMARY KEY (app_id, position)
    ) STRICT;

    CREATE TABLE favorites (
        app_id     TEXT PRIMARY KEY NOT NULL REFERENCES installed_apps (id) ON DELETE CASCADE,
        created_at INTEGER NOT NULL
    ) STRICT;
    CREATE INDEX idx_favorites_created ON favorites (created_at, app_id);

    CREATE TABLE settings (
        key        TEXT PRIMARY KEY NOT NULL,
        kind       TEXT NOT NULL CHECK (kind IN ('bool', 'int', 'text')),
        int_value  INTEGER,
        text_value TEXT
    ) STRICT;

    CREATE TABLE app_settings (
        app_id     TEXT NOT NULL REFERENCES installed_apps (id) ON DELETE CASCADE,
        key        TEXT NOT NULL,
        kind       TEXT NOT NULL CHECK (kind IN ('bool', 'int', 'text')),
        int_value  INTEGER,
        text_value TEXT,
        PRIMARY KEY (app_id, key)
    ) STRICT;
    ",
    // 2: profiles, history, permission decisions.
    "
    CREATE TABLE profiles (
        id         INTEGER PRIMARY KEY,
        kind       TEXT NOT NULL CHECK (kind IN ('default', 'persistent')),
        profile_id TEXT UNIQUE,
        name       TEXT NOT NULL,
        created_at INTEGER NOT NULL,
        CHECK ((kind = 'default') = (profile_id IS NULL))
    ) STRICT;
    INSERT INTO profiles (id, kind, profile_id, name, created_at)
        VALUES (1, 'default', NULL, 'Default', 0);

    CREATE TABLE history (
        id          INTEGER PRIMARY KEY,
        profile_ref INTEGER NOT NULL REFERENCES profiles (id) ON DELETE CASCADE,
        url         TEXT NOT NULL,
        title       TEXT,
        visited_at  INTEGER NOT NULL
    ) STRICT;
    CREATE INDEX idx_history_profile_visited ON history (profile_ref, visited_at, id);

    CREATE TABLE permissions (
        profile_ref INTEGER NOT NULL REFERENCES profiles (id) ON DELETE CASCADE,
        origin      TEXT NOT NULL,
        kind        TEXT NOT NULL,
        decision    TEXT NOT NULL CHECK (decision IN ('grant', 'deny')),
        decided_at  INTEGER NOT NULL,
        PRIMARY KEY (profile_ref, origin, kind)
    ) STRICT;
    ",
];

/// The schema version this build produces.
pub(crate) fn latest() -> u32 {
    u32::try_from(MIGRATIONS.len()).unwrap_or(u32::MAX)
}

pub(crate) fn user_version(conn: &Connection) -> Result<u32> {
    Ok(conn.pragma_query_value(None, "user_version", |r| r.get(0))?)
}

/// Applies pending migrations up to `target`, each in its own immediate transaction that
/// also bumps `user_version`, so a failure leaves the previous version intact.
pub(crate) fn migrate_to(conn: &mut Connection, target: u32) -> Result<()> {
    loop {
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let current = user_version(&tx)?;
        if current > latest() {
            return Err(StorageError::SchemaTooNew {
                found: current,
                supported: latest(),
            });
        }
        if current >= target {
            return Ok(());
        }
        let next = current + 1;
        let sql = MIGRATIONS[current as usize];
        let step = tx
            .execute_batch(sql)
            .and_then(|()| tx.pragma_update(None, "user_version", next));
        step.map_err(|source| StorageError::Migration {
            version: next,
            source,
        })?;
        tx.commit()?;
    }
}
