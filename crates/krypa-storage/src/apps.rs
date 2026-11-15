//! Installed apps and favorites.
//!
//! An `InstalledApp` is rebuilt from stored text with `InstalledApp::from_stored`, which
//! re-validates every invariant (origin binding, scope, name, icons) instead of trusting
//! the database; a row that fails, or whose text is not exactly what this code writes,
//! is corrupt.

use krypa_apps::{
    sanitize_name, DisplayMode, Icon, IconPurposes, IconSize, InstalledApp, Rgba, StoredApp,
    StoredIcon,
};
use rusqlite::OptionalExtension;
use url::Url;

use crate::error::{corrupt, is_constraint, read_err, Result, StorageError};
use crate::text::{clamp_limit, fetch_size, into_page, Page};
use crate::{Storage, UnixMillis};

const APPS: &str = "installed_apps";

/// What a list of apps needs: no icons, colours or scope.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppSummary {
    /// Stable app identity.
    pub id: Url,
    /// Sanitized display name.
    pub name: String,
    /// URL the app opens at.
    pub start_url: Url,
    /// Display mode.
    pub display: DisplayMode,
}

/// One page of complete installed apps. Rows that failed validation are in `errors`
/// instead of `apps`, so a single corrupt row never hides the others.
#[derive(Debug)]
pub struct InstalledAppsPage {
    /// The rows that validated, in list order.
    pub apps: Vec<InstalledApp>,
    /// One [`StorageError::CorruptRow`] per row that did not validate.
    pub errors: Vec<StorageError>,
    /// Pass to `list_installed_apps` for the next page; `None` at the end.
    pub next: Option<AppCursor>,
}

/// Position in the app list (ordered by name, then id).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppCursor {
    name: String,
    id: String,
}

/// Position in the favorites list (newest first).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FavoriteCursor {
    created_at: UnixMillis,
    id: String,
}

fn display_str(d: DisplayMode) -> &'static str {
    match d {
        DisplayMode::Standalone => "standalone",
        DisplayMode::MinimalUi => "minimal-ui",
        DisplayMode::Browser => "browser",
    }
}

fn parse_display(s: &str) -> Option<DisplayMode> {
    match s {
        "standalone" => Some(DisplayMode::Standalone),
        "minimal-ui" => Some(DisplayMode::MinimalUi),
        "browser" => Some(DisplayMode::Browser),
        _ => None,
    }
}

fn color_str(c: Option<Rgba>) -> Option<String> {
    c.map(|c| format!("#{:02x}{:02x}{:02x}{:02x}", c.r, c.g, c.b, c.a))
}

/// `(src, sizes, purposes)` as stored.
type IconRow = (String, String, String);

fn encode_icon(icon: &Icon) -> IconRow {
    encode_stored_icon(&StoredIcon {
        src: icon.src().as_str().to_owned(),
        sizes: icon.sizes().to_vec(),
        purposes: icon.purposes(),
    })
}

struct RawApp {
    rowid: i64,
    id: String,
    name: String,
    start_url: String,
    scope: String,
    display: String,
    theme: Option<String>,
    background: Option<String>,
}

const RAW_APP_COLUMNS: &str =
    "rowid, id, name, start_url, scope, display, theme_color, background_color";

fn raw_app(r: &rusqlite::Row<'_>) -> rusqlite::Result<RawApp> {
    Ok(RawApp {
        rowid: r.get(0)?,
        id: r.get(1)?,
        name: r.get(2)?,
        start_url: r.get(3)?,
        scope: r.get(4)?,
        display: r.get(5)?,
        theme: r.get(6)?,
        background: r.get(7)?,
    })
}

/// Strict inverse of `color_str`: only `#rrggbbaa` in lower-case hex.
fn parse_color(text: &str) -> Option<Rgba> {
    let hex = text.strip_prefix('#')?;
    let canonical = hex.len() == 8 && hex.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
    let channel = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();
    if !canonical {
        return None;
    }
    Some(Rgba {
        r: channel(0)?,
        g: channel(2)?,
        b: channel(4)?,
        a: channel(6)?,
    })
}

fn decode_icon((src, sizes, purposes): &IconRow) -> Option<StoredIcon> {
    let sizes_parsed = sizes
        .split(' ')
        .filter(|t| !t.is_empty())
        .map(|t| match t {
            "any" => Some(IconSize::Any),
            _ => {
                let (w, h) = t.split_once('x')?;
                Some(IconSize::Px {
                    width: w.parse().ok()?,
                    height: h.parse().ok()?,
                })
            }
        })
        .collect::<Option<Vec<_>>>()?;
    let mut p = IconPurposes {
        any: false,
        maskable: false,
        monochrome: false,
    };
    for t in purposes.split(' ').filter(|t| !t.is_empty()) {
        match t {
            "any" => p.any = true,
            "maskable" => p.maskable = true,
            "monochrome" => p.monochrome = true,
            _ => return None,
        }
    }
    let icon = StoredIcon {
        src: src.clone(),
        sizes: sizes_parsed,
        purposes: p,
    };
    // Only the exact text this code writes is accepted: anything that would be rewritten
    // on the way back (extra spaces, `048x48`, repeated tokens) means the row is foreign.
    (encode_stored_icon(&icon) == (src.clone(), sizes.clone(), purposes.clone())).then_some(icon)
}

fn encode_stored_icon(icon: &StoredIcon) -> IconRow {
    let sizes = icon
        .sizes
        .iter()
        .map(|s| match s {
            IconSize::Any => "any".to_owned(),
            IconSize::Px { width, height } => format!("{width}x{height}"),
        })
        .collect::<Vec<_>>()
        .join(" ");
    let p = icon.purposes;
    let purposes = [
        (p.any, "any"),
        (p.maskable, "maskable"),
        (p.monochrome, "monochrome"),
    ]
    .into_iter()
    .filter_map(|(on, name)| on.then_some(name))
    .collect::<Vec<_>>()
    .join(" ");
    (icon.src.clone(), sizes, purposes)
}

/// Rebuilds a domain object from stored text. Nothing is trusted: every field is decoded
/// strictly and the result goes through `InstalledApp::from_stored`, which enforces the
/// same invariants as installing from a manifest.
fn rebuild(raw: &RawApp, icons: &[IconRow]) -> Result<InstalledApp> {
    let bad = |field| corrupt(APPS, Some(raw.rowid), field);
    let color = |text: &Option<String>, field| match text {
        None => Ok(None),
        Some(t) => parse_color(t).map(Some).ok_or_else(|| bad(field)),
    };
    let stored = StoredApp {
        id: raw.id.clone(),
        name: raw.name.clone(),
        start_url: raw.start_url.clone(),
        scope: raw.scope.clone(),
        display: parse_display(&raw.display).ok_or_else(|| bad("display"))?,
        theme_color: color(&raw.theme, "theme_color")?,
        background_color: color(&raw.background, "background_color")?,
        icons: icons
            .iter()
            .map(|i| decode_icon(i).ok_or_else(|| bad("icons")))
            .collect::<Result<_>>()?,
    };
    InstalledApp::from_stored(stored).map_err(|e| bad(e.field))
}

fn plain_web_url(text: &str, field: &'static str) -> Result<Url> {
    let url = Url::parse(text).map_err(|_| corrupt(APPS, None, field))?;
    let plain = matches!(url.scheme(), "http" | "https")
        && url.host_str().is_some()
        && url.username().is_empty()
        && url.password().is_none();
    plain
        .then_some(url)
        .ok_or_else(|| corrupt(APPS, None, field))
}

fn summary(id: &str, name: String, start_url: &str, display: &str) -> Result<AppSummary> {
    if name.is_empty() || sanitize_name(&name) != name {
        return Err(corrupt(APPS, None, "name"));
    }
    Ok(AppSummary {
        id: plain_web_url(id, "id")?,
        name,
        start_url: plain_web_url(start_url, "start_url")?,
        display: parse_display(display).ok_or_else(|| corrupt(APPS, None, "display"))?,
    })
}

type SummaryRow = (String, String, String, String, UnixMillis);

fn collect_summaries(rows: Vec<SummaryRow>) -> Result<Vec<(AppSummary, UnixMillis)>> {
    rows.into_iter()
        .map(|(id, name, start, display, at)| Ok((summary(&id, name, &start, &display)?, at)))
        .collect()
}

impl Storage {
    /// Inserts or updates an installed app and replaces its icons, atomically. Updating
    /// keeps the app's favorite and settings.
    ///
    /// # Errors
    /// [`StorageError::Sqlite`] on database failure.
    pub fn save_app(&self, app: &InstalledApp, installed_at: UnixMillis) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "INSERT INTO installed_apps
                 (id, name, start_url, scope, display, theme_color, background_color, installed_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(id) DO UPDATE SET
                 name = excluded.name, start_url = excluded.start_url, scope = excluded.scope,
                 display = excluded.display, theme_color = excluded.theme_color,
                 background_color = excluded.background_color",
            rusqlite::params![
                app.id().as_str(),
                app.name(),
                app.start_url().as_str(),
                app.scope().as_str(),
                display_str(app.display()),
                color_str(app.theme_color()),
                color_str(app.background_color()),
                installed_at,
            ],
        )?;
        tx.execute(
            "DELETE FROM app_icons WHERE app_id = ?1",
            [app.id().as_str()],
        )?;
        for (position, icon) in app.icons().iter().enumerate() {
            let (src, sizes, purposes) = encode_icon(icon);
            tx.execute(
                "INSERT INTO app_icons (app_id, position, src, sizes, purposes)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                rusqlite::params![app.id().as_str(), position as i64, src, sizes, purposes],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    fn load_icons(&self, id: &str) -> Result<Vec<IconRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT src, sizes, purposes FROM app_icons WHERE app_id = ?1 ORDER BY position",
        )?;
        let icons = stmt
            .query_map([id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<std::result::Result<Vec<IconRow>, _>>()
            .map_err(read_err("app_icons"))?;
        Ok(icons)
    }

    /// Loads one app, rebuilt through `InstalledApp::from_stored` so every invariant is
    /// re-checked; `None` if no app has this id.
    ///
    /// # Errors
    /// [`StorageError::CorruptRow`] if the stored row does not validate.
    pub fn get_app(&self, id: &Url) -> Result<Option<InstalledApp>> {
        let raw = self
            .conn
            .query_row(
                &format!("SELECT {RAW_APP_COLUMNS} FROM installed_apps WHERE id = ?1"),
                [id.as_str()],
                raw_app,
            )
            .optional()
            .map_err(read_err(APPS))?;
        let Some(raw) = raw else { return Ok(None) };
        let icons = self.load_icons(id.as_str())?;
        rebuild(&raw, &icons).map(Some)
    }

    /// Loads one page of complete installed apps, ordered by name, then id (the same
    /// order and cursor as [`Storage::list_apps`]).
    ///
    /// A row that fails validation neither fails the page nor is dropped silently: it is
    /// reported in [`InstalledAppsPage::errors`] (a [`StorageError::CorruptRow`] carrying
    /// the SQLite rowid) while the valid rows are returned, and paging continues past it.
    /// Database failures still return `Err`.
    ///
    /// # Errors
    /// [`StorageError::Sqlite`] on database failure.
    pub fn list_installed_apps(
        &self,
        limit: usize,
        after: Option<&AppCursor>,
    ) -> Result<InstalledAppsPage> {
        let limit = clamp_limit(limit);
        let size = fetch_size(limit);
        let rows: Vec<RawApp> = match after {
            None => self
                .conn
                .prepare(&format!(
                    "SELECT {RAW_APP_COLUMNS} FROM installed_apps ORDER BY name, id LIMIT ?1"
                ))?
                .query_map([size], raw_app)?
                .collect::<std::result::Result<_, _>>(),
            Some(c) => self
                .conn
                .prepare(&format!(
                    "SELECT {RAW_APP_COLUMNS} FROM installed_apps
                     WHERE (name, id) > (?1, ?2) ORDER BY name, id LIMIT ?3"
                ))?
                .query_map(rusqlite::params![c.name, c.id, size], raw_app)?
                .collect::<std::result::Result<_, _>>(),
        }
        .map_err(read_err(APPS))?;
        let page = into_page(rows, limit, |r: &RawApp| AppCursor {
            name: r.name.clone(),
            id: r.id.clone(),
        });
        let mut apps = Vec::new();
        let mut errors = Vec::new();
        for raw in &page.items {
            let icons = self.load_icons(&raw.id)?;
            match rebuild(raw, &icons) {
                Ok(app) => apps.push(app),
                Err(e @ StorageError::CorruptRow { .. }) => errors.push(e),
                Err(e) => return Err(e),
            }
        }
        Ok(InstalledAppsPage {
            apps,
            errors,
            next: page.next,
        })
    }

    /// Lists apps ordered by name, then id.
    ///
    /// # Errors
    /// [`StorageError::CorruptRow`] if a row on the page does not validate.
    pub fn list_apps(
        &self,
        limit: usize,
        after: Option<&AppCursor>,
    ) -> Result<Page<AppSummary, AppCursor>> {
        let limit = clamp_limit(limit);
        let size = fetch_size(limit);
        let map = |r: &rusqlite::Row<'_>| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, 0_i64));
        let rows: Vec<SummaryRow> = match after {
            None => self
                .conn
                .prepare(
                    "SELECT id, name, start_url, display, 0 FROM installed_apps
                     ORDER BY name, id LIMIT ?1",
                )?
                .query_map([size], map)?
                .collect::<std::result::Result<_, _>>(),
            Some(c) => self
                .conn
                .prepare(
                    "SELECT id, name, start_url, display, 0 FROM installed_apps
                     WHERE (name, id) > (?1, ?2) ORDER BY name, id LIMIT ?3",
                )?
                .query_map(rusqlite::params![c.name, c.id, size], map)?
                .collect::<std::result::Result<_, _>>(),
        }
        .map_err(read_err(APPS))?;
        let items = collect_summaries(rows)?
            .into_iter()
            .map(|(s, _)| s)
            .collect();
        Ok(into_page(items, limit, |a: &AppSummary| AppCursor {
            name: a.name.clone(),
            id: a.id.as_str().to_owned(),
        }))
    }

    /// Deletes an app together with its icons, favorite and app-specific settings.
    /// Returns whether it existed.
    ///
    /// # Errors
    /// [`StorageError::Sqlite`] on database failure.
    pub fn delete_app(&self, id: &Url) -> Result<bool> {
        let n = self
            .conn
            .execute("DELETE FROM installed_apps WHERE id = ?1", [id.as_str()])?;
        Ok(n > 0)
    }

    /// Marks an installed app as favorite (idempotent).
    ///
    /// # Errors
    /// [`StorageError::UnknownApp`] if the app is not installed.
    pub fn add_favorite(&self, id: &Url, created_at: UnixMillis) -> Result<()> {
        match self.conn.execute(
            "INSERT INTO favorites (app_id, created_at) VALUES (?1, ?2)
             ON CONFLICT(app_id) DO NOTHING",
            rusqlite::params![id.as_str(), created_at],
        ) {
            Ok(_) => Ok(()),
            Err(e) if is_constraint(&e) => Err(StorageError::UnknownApp),
            Err(e) => Err(e.into()),
        }
    }

    /// Removes a favorite. Returns whether it existed.
    ///
    /// # Errors
    /// [`StorageError::Sqlite`] on database failure.
    pub fn remove_favorite(&self, id: &Url) -> Result<bool> {
        let n = self
            .conn
            .execute("DELETE FROM favorites WHERE app_id = ?1", [id.as_str()])?;
        Ok(n > 0)
    }

    /// Whether the app is a favorite.
    ///
    /// # Errors
    /// [`StorageError::Sqlite`] on database failure.
    pub fn is_favorite(&self, id: &Url) -> Result<bool> {
        Ok(self.conn.query_row(
            "SELECT EXISTS (SELECT 1 FROM favorites WHERE app_id = ?1)",
            [id.as_str()],
            |r| r.get(0),
        )?)
    }

    /// Lists favorite apps, most recently favorited first.
    ///
    /// # Errors
    /// [`StorageError::CorruptRow`] if a row on the page does not validate.
    pub fn list_favorites(
        &self,
        limit: usize,
        after: Option<&FavoriteCursor>,
    ) -> Result<Page<AppSummary, FavoriteCursor>> {
        let limit = clamp_limit(limit);
        let size = fetch_size(limit);
        let map =
            |r: &rusqlite::Row<'_>| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?));
        let rows: Vec<SummaryRow> = match after {
            None => self
                .conn
                .prepare(
                    "SELECT a.id, a.name, a.start_url, a.display, f.created_at
                     FROM favorites f JOIN installed_apps a ON a.id = f.app_id
                     ORDER BY f.created_at DESC, f.app_id DESC LIMIT ?1",
                )?
                .query_map([size], map)?
                .collect::<std::result::Result<_, _>>(),
            Some(c) => self
                .conn
                .prepare(
                    "SELECT a.id, a.name, a.start_url, a.display, f.created_at
                     FROM favorites f JOIN installed_apps a ON a.id = f.app_id
                     WHERE (f.created_at, f.app_id) < (?1, ?2)
                     ORDER BY f.created_at DESC, f.app_id DESC LIMIT ?3",
                )?
                .query_map(rusqlite::params![c.created_at, c.id, size], map)?
                .collect::<std::result::Result<_, _>>(),
        }
        .map_err(read_err("favorites"))?;
        let items = collect_summaries(rows)?;
        Ok(into_page(items, limit, |(s, at)| FavoriteCursor {
            created_at: *at,
            id: s.id.as_str().to_owned(),
        })
        .map_items(|(s, _)| s))
    }
}

impl<T, C> Page<T, C> {
    fn map_items<U>(self, f: impl FnMut(T) -> U) -> Page<U, C> {
        Page {
            items: self.items.into_iter().map(f).collect(),
            next: self.next,
        }
    }
}
