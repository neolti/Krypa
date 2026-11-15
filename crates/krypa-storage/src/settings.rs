//! Typed key/value settings, global and per installed app.

use rusqlite::OptionalExtension;
use url::Url;

use crate::error::{corrupt, is_constraint, read_err, Result, StorageError};
use crate::text::{clamp_limit, fetch_size, into_page, Page};
use crate::Storage;

/// Largest text setting, in bytes.
pub const MAX_SETTING_TEXT_BYTES: usize = 8192;
const MAX_KEY_BYTES: usize = 128;

/// A setting value. The type is stored with the value, so a bool never reads back as an
/// integer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingValue {
    /// A flag.
    Bool(bool),
    /// An integer.
    Int(i64),
    /// Text (at most [`MAX_SETTING_TEXT_BYTES`] bytes).
    Text(String),
}

fn check_key(key: &str) -> Result<()> {
    let ok = !key.is_empty()
        && key.len() <= MAX_KEY_BYTES
        && key.bytes().all(|b| {
            b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'.' | b'_' | b'-')
        });
    if ok {
        Ok(())
    } else {
        Err(StorageError::InvalidInput(
            "setting key must be 1-128 bytes of [a-z0-9._-]",
        ))
    }
}

/// `(kind, int_value, text_value)` as stored.
type Cells = (&'static str, Option<i64>, Option<String>);

fn encode(value: &SettingValue) -> Result<Cells> {
    Ok(match value {
        SettingValue::Bool(b) => ("bool", Some(i64::from(*b)), None),
        SettingValue::Int(i) => ("int", Some(*i), None),
        SettingValue::Text(t) => {
            if t.len() > MAX_SETTING_TEXT_BYTES {
                return Err(StorageError::InvalidInput("setting text is too long"));
            }
            ("text", None, Some(t.clone()))
        }
    })
}

fn decode(
    table: &'static str,
    kind: &str,
    int: Option<i64>,
    text: Option<String>,
) -> Result<SettingValue> {
    let bad = || corrupt(table, None, "value");
    match (kind, int, text) {
        ("bool", Some(0), None) => Ok(SettingValue::Bool(false)),
        ("bool", Some(1), None) => Ok(SettingValue::Bool(true)),
        ("int", Some(i), None) => Ok(SettingValue::Int(i)),
        ("text", None, Some(t)) if t.len() <= MAX_SETTING_TEXT_BYTES => Ok(SettingValue::Text(t)),
        _ => Err(bad()),
    }
}

type RawSetting = (String, String, Option<i64>, Option<String>);

fn decode_all(table: &'static str, rows: Vec<RawSetting>) -> Result<Vec<(String, SettingValue)>> {
    rows.into_iter()
        .map(|(key, kind, int, text)| {
            check_key(&key).map_err(|_| corrupt(table, None, "key"))?;
            Ok((key, decode(table, &kind, int, text)?))
        })
        .collect()
}

impl Storage {
    /// Sets a global setting.
    ///
    /// # Errors
    /// [`StorageError::InvalidInput`] for a bad key or oversized text.
    pub fn set_setting(&self, key: &str, value: &SettingValue) -> Result<()> {
        check_key(key)?;
        let (kind, int, text) = encode(value)?;
        self.conn.execute(
            "INSERT INTO settings (key, kind, int_value, text_value) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(key) DO UPDATE SET
                 kind = excluded.kind, int_value = excluded.int_value,
                 text_value = excluded.text_value",
            rusqlite::params![key, kind, int, text],
        )?;
        Ok(())
    }

    /// Reads a global setting.
    ///
    /// # Errors
    /// [`StorageError::CorruptRow`] if the stored value is inconsistent.
    pub fn get_setting(&self, key: &str) -> Result<Option<SettingValue>> {
        check_key(key)?;
        let raw = self
            .conn
            .query_row(
                "SELECT kind, int_value, text_value FROM settings WHERE key = ?1",
                [key],
                |r| Ok((r.get::<_, String>(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()
            .map_err(read_err("settings"))?;
        raw.map(|(kind, int, text)| decode("settings", &kind, int, text))
            .transpose()
    }

    /// Deletes a global setting. Returns whether it existed.
    ///
    /// # Errors
    /// [`StorageError::InvalidInput`] for a bad key.
    pub fn delete_setting(&self, key: &str) -> Result<bool> {
        check_key(key)?;
        Ok(self
            .conn
            .execute("DELETE FROM settings WHERE key = ?1", [key])?
            > 0)
    }

    /// Lists global settings ordered by key; `after` is the last key of the previous page.
    ///
    /// # Errors
    /// [`StorageError::CorruptRow`] if a row does not validate.
    pub fn list_settings(
        &self,
        limit: usize,
        after: Option<&str>,
    ) -> Result<Page<(String, SettingValue), String>> {
        let limit = clamp_limit(limit);
        let rows = self
            .conn
            .prepare(
                "SELECT key, kind, int_value, text_value FROM settings
                 WHERE key > ?1 ORDER BY key LIMIT ?2",
            )?
            .query_map(
                rusqlite::params![after.unwrap_or(""), fetch_size(limit)],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )?
            .collect::<std::result::Result<Vec<RawSetting>, _>>()
            .map_err(read_err("settings"))?;
        Ok(into_page(decode_all("settings", rows)?, limit, |(k, _)| {
            k.clone()
        }))
    }

    /// Sets a setting that belongs to one installed app; it is deleted with the app.
    ///
    /// # Errors
    /// [`StorageError::UnknownApp`] if the app is not installed.
    pub fn set_app_setting(&self, app: &Url, key: &str, value: &SettingValue) -> Result<()> {
        check_key(key)?;
        let (kind, int, text) = encode(value)?;
        match self.conn.execute(
            "INSERT INTO app_settings (app_id, key, kind, int_value, text_value)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(app_id, key) DO UPDATE SET
                 kind = excluded.kind, int_value = excluded.int_value,
                 text_value = excluded.text_value",
            rusqlite::params![app.as_str(), key, kind, int, text],
        ) {
            Ok(_) => Ok(()),
            Err(e) if is_constraint(&e) => Err(StorageError::UnknownApp),
            Err(e) => Err(e.into()),
        }
    }

    /// Reads an app-specific setting.
    ///
    /// # Errors
    /// [`StorageError::CorruptRow`] if the stored value is inconsistent.
    pub fn get_app_setting(&self, app: &Url, key: &str) -> Result<Option<SettingValue>> {
        check_key(key)?;
        let raw = self
            .conn
            .query_row(
                "SELECT kind, int_value, text_value FROM app_settings
                 WHERE app_id = ?1 AND key = ?2",
                rusqlite::params![app.as_str(), key],
                |r| Ok((r.get::<_, String>(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()
            .map_err(read_err("app_settings"))?;
        raw.map(|(kind, int, text)| decode("app_settings", &kind, int, text))
            .transpose()
    }

    /// Lists the settings of one app ordered by key.
    ///
    /// # Errors
    /// [`StorageError::CorruptRow`] if a row does not validate.
    pub fn list_app_settings(
        &self,
        app: &Url,
        limit: usize,
        after: Option<&str>,
    ) -> Result<Page<(String, SettingValue), String>> {
        let limit = clamp_limit(limit);
        let rows = self
            .conn
            .prepare(
                "SELECT key, kind, int_value, text_value FROM app_settings
                 WHERE app_id = ?1 AND key > ?2 ORDER BY key LIMIT ?3",
            )?
            .query_map(
                rusqlite::params![app.as_str(), after.unwrap_or(""), fetch_size(limit)],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )?
            .collect::<std::result::Result<Vec<RawSetting>, _>>()
            .map_err(read_err("app_settings"))?;
        Ok(into_page(
            decode_all("app_settings", rows)?,
            limit,
            |(k, _)| k.clone(),
        ))
    }
}
