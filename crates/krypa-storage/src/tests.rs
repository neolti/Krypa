//! Tests run against a real SQLite file in a temporary directory.

use std::path::PathBuf;

use krypa_apps::{InstalledApp, Manifest};
use krypa_engine_api::{PermissionDecision, PermissionKind, ProfileId, ProfileKind};
use krypa_navigation::NavigationUrl;
use rusqlite::Connection;
use tempfile::TempDir;
use url::Url;

use crate::migrations::{self, latest};
use crate::{Persisted, SettingValue, Storage, StorageError, TimeRange};

struct Fixture {
    dir: TempDir,
}

impl Fixture {
    fn new() -> Self {
        Self {
            dir: tempfile::tempdir().unwrap(),
        }
    }
    fn path(&self) -> PathBuf {
        self.dir.path().join("krypa.sqlite3")
    }
    fn open(&self) -> Storage {
        Storage::open(&self.path()).unwrap()
    }
    /// A second connection that tampers with the file behind the API's back.
    fn raw(&self) -> Connection {
        let c = Connection::open(self.path()).unwrap();
        c.pragma_update(None, "ignore_check_constraints", true)
            .unwrap();
        c.pragma_update(None, "foreign_keys", false).unwrap();
        c
    }
}

fn nav(s: &str) -> NavigationUrl {
    NavigationUrl::parse(s).unwrap()
}

fn url(s: &str) -> Url {
    Url::parse(s).unwrap()
}

fn manual_app(site: &str, title: &str) -> InstalledApp {
    InstalledApp::manual(&url(site), Some(title)).unwrap()
}

fn rich_app() -> InstalledApp {
    let m = Manifest::parse(
        r##"{
            "name": "Notes", "id": "/notes", "start_url": "/notes/home?x=1#top",
            "scope": "/notes/", "display": "minimal-ui",
            "theme_color": "#112233", "background_color": "rgba(1,2,3,0.5)",
            "icons": [
                {"src": "/i/192.png", "sizes": "192x192 any", "purpose": "any maskable"},
                {"src": "/i/mono.png", "sizes": "48x48", "purpose": "monochrome"},
                {"src": "http://insecure.example/x.png", "sizes": "48x48"}
            ]
        }"##,
    )
    .unwrap();
    let doc = url("https://notes.example/notes/");
    InstalledApp::from_manifest(&doc, &url("https://notes.example/m.json"), &m).unwrap()
}

fn origin(s: &str) -> krypa_navigation::Origin {
    nav(s).origin()
}

fn persistent(id: &str) -> ProfileKind {
    ProfileKind::Persistent(ProfileId(id.to_owned()))
}

fn count(conn: &Connection, table: &str) -> i64 {
    conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
        .unwrap()
}

fn schema(conn: &Connection) -> Vec<(String, String, Option<String>)> {
    let mut stmt = conn
        .prepare("SELECT type, name, sql FROM sqlite_master ORDER BY type, name")
        .unwrap();
    stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

fn assert_fk_clean(f: &Fixture) {
    let conn = f.raw();
    let mut stmt = conn.prepare("PRAGMA foreign_key_check").unwrap();
    let violations = stmt.query_map([], |_| Ok(())).unwrap().count();
    assert_eq!(violations, 0, "foreign_key_check must be clean");
}

// ---- migrations ---------------------------------------------------------------------

#[test]
fn opening_an_empty_file_migrates_to_the_latest_version() {
    let f = Fixture::new();
    let s = f.open();
    assert_eq!(s.schema_version().unwrap(), latest());
    assert!(latest() >= 2, "the upgrade-path tests need real history");
}

#[test]
fn migrating_from_every_earlier_version_reaches_the_same_schema_as_a_fresh_file() {
    let fresh = Fixture::new();
    drop(fresh.open());
    let expected = schema(&fresh.raw());

    for from in 0..latest() {
        let f = Fixture::new();
        {
            let mut conn = Connection::open(f.path()).unwrap();
            migrations::migrate_to(&mut conn, from).unwrap();
            assert_eq!(migrations::user_version(&conn).unwrap(), from);
        }
        let s = f.open();
        assert_eq!(s.schema_version().unwrap(), latest(), "from v{from}");
        assert_eq!(
            schema(&f.raw()),
            expected,
            "schema differs when upgrading from v{from}"
        );
    }
}

#[test]
fn data_written_under_an_older_version_survives_the_upgrade() {
    let f = Fixture::new();
    {
        let mut conn = Connection::open(f.path()).unwrap();
        migrations::migrate_to(&mut conn, 1).unwrap();
        conn.execute(
            "INSERT INTO settings (key, kind, int_value) VALUES ('ui.zoom', 'int', 3)",
            [],
        )
        .unwrap();
    }
    let s = f.open();
    assert_eq!(
        s.get_setting("ui.zoom").unwrap(),
        Some(SettingValue::Int(3))
    );
}

#[test]
fn a_database_from_a_newer_version_is_refused_and_left_untouched() {
    let f = Fixture::new();
    {
        let conn = Connection::open(f.path()).unwrap();
        conn.pragma_update(None, "user_version", latest() + 7)
            .unwrap();
    }
    let err = Storage::open(&f.path()).unwrap_err();
    assert!(
        matches!(err, StorageError::SchemaTooNew { found, supported }
            if found == latest() + 7 && supported == latest()),
        "{err:?}"
    );
    let conn = f.raw();
    assert_eq!(migrations::user_version(&conn).unwrap(), latest() + 7);
    assert!(
        schema(&conn).is_empty(),
        "no table may be created in a newer file"
    );
}

#[test]
fn a_failing_migration_rolls_back_and_keeps_the_previous_version() {
    let f = Fixture::new();
    let mut conn = Connection::open(f.path()).unwrap();
    migrations::migrate_to(&mut conn, 1).unwrap();
    // Block migration 2 by squatting on one of its table names.
    conn.execute_batch("CREATE TABLE history (x)").unwrap();
    let err = migrations::migrate_to(&mut conn, 2).unwrap_err();
    assert!(
        matches!(err, StorageError::Migration { version: 2, .. }),
        "{err:?}"
    );
    assert_eq!(migrations::user_version(&conn).unwrap(), 1);
    let profiles: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE name = 'profiles'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(profiles, 0, "a half-applied migration must not remain");
}

#[test]
fn foreign_keys_and_wal_are_on_for_every_connection_opened() {
    let f = Fixture::new();
    let s = f.open();
    let fk: i64 = s
        .conn
        .pragma_query_value(None, "foreign_keys", |r| r.get(0))
        .unwrap();
    let mode: String = s
        .conn
        .pragma_query_value(None, "journal_mode", |r| r.get(0))
        .unwrap();
    assert_eq!(fk, 1);
    assert_eq!(mode, "wal");
}

// ---- persistence --------------------------------------------------------------------

#[test]
fn reopening_the_file_returns_everything_that_was_stored() {
    let f = Fixture::new();
    let app = rich_app();
    let other = manual_app("https://other.example/a/b.html", "Other");
    {
        let s = f.open();
        s.save_app(&app, 10).unwrap();
        s.save_app(&other, 11).unwrap();
        s.add_favorite(app.id(), 20).unwrap();
        s.set_setting("ui.dark", &SettingValue::Bool(true)).unwrap();
        s.set_app_setting(app.id(), "notify", &SettingValue::Text("daily".into()))
            .unwrap();
        s.create_profile(&ProfileId("work".into()), "Work", 5)
            .unwrap();
        s.record_visit(
            &persistent("work"),
            &nav("https://a.example/p?q=1"),
            Some("A page"),
            30,
        )
        .unwrap();
        s.set_permission(
            &ProfileKind::Default,
            &origin("https://cam.example"),
            PermissionKind::Camera,
            PermissionDecision::Grant,
            40,
        )
        .unwrap();
    }
    let s = f.open();
    assert_eq!(
        s.get_app(app.id()).unwrap().unwrap(),
        app,
        "icons, colours and scope round-trip"
    );
    assert_eq!(s.get_app(other.id()).unwrap().unwrap(), other);
    assert!(s.is_favorite(app.id()).unwrap());
    assert_eq!(
        s.get_setting("ui.dark").unwrap(),
        Some(SettingValue::Bool(true))
    );
    assert_eq!(
        s.get_app_setting(app.id(), "notify").unwrap(),
        Some(SettingValue::Text("daily".into()))
    );
    let profiles = s.list_profiles(10, None).unwrap();
    assert_eq!(profiles.items.len(), 1);
    assert_eq!(profiles.items[0].name, "Work");
    let history = s.list_history(&persistent("work"), 10, None).unwrap();
    assert_eq!(history.items.len(), 1);
    assert_eq!(history.items[0].url, nav("https://a.example/p?q=1"));
    assert_eq!(history.items[0].title.as_deref(), Some("A page"));
    assert_eq!(
        s.get_permission(
            &ProfileKind::Default,
            &origin("https://cam.example"),
            PermissionKind::Camera
        )
        .unwrap(),
        Some(PermissionDecision::Grant)
    );
}

#[test]
fn saving_an_app_again_updates_it_without_losing_its_favorite_or_settings() {
    let f = Fixture::new();
    let s = f.open();
    let first = manual_app("https://x.example/", "Old name");
    s.save_app(&first, 1).unwrap();
    s.add_favorite(first.id(), 2).unwrap();
    s.set_app_setting(first.id(), "k", &SettingValue::Int(1))
        .unwrap();
    let renamed = manual_app("https://x.example/", "New name");
    s.save_app(&renamed, 3).unwrap();
    assert_eq!(s.get_app(first.id()).unwrap().unwrap().name(), "New name");
    assert!(s.is_favorite(first.id()).unwrap());
    assert_eq!(
        s.get_app_setting(first.id(), "k").unwrap(),
        Some(SettingValue::Int(1))
    );
}

#[test]
fn settings_keep_their_type_so_a_bool_never_reads_back_as_an_int() {
    let f = Fixture::new();
    let s = f.open();
    s.set_setting("a", &SettingValue::Bool(true)).unwrap();
    s.set_setting("b", &SettingValue::Int(1)).unwrap();
    s.set_setting("c", &SettingValue::Text("1".into())).unwrap();
    assert_eq!(s.get_setting("a").unwrap(), Some(SettingValue::Bool(true)));
    assert_eq!(s.get_setting("b").unwrap(), Some(SettingValue::Int(1)));
    assert_eq!(
        s.get_setting("c").unwrap(),
        Some(SettingValue::Text("1".into()))
    );
    s.set_setting("a", &SettingValue::Int(-5)).unwrap();
    assert_eq!(s.get_setting("a").unwrap(), Some(SettingValue::Int(-5)));
    assert!(s.delete_setting("a").unwrap());
    assert_eq!(s.get_setting("a").unwrap(), None);
}

#[test]
fn setting_keys_and_values_that_could_abuse_the_store_are_rejected() {
    let f = Fixture::new();
    let s = f.open();
    for key in ["", "UPPER", "has space", "semi;colon", &"k".repeat(129)] {
        assert!(
            matches!(
                s.set_setting(key, &SettingValue::Bool(true)),
                Err(StorageError::InvalidInput(_))
            ),
            "key {key:?}"
        );
    }
    let big = SettingValue::Text("x".repeat(crate::MAX_SETTING_TEXT_BYTES + 1));
    assert!(matches!(
        s.set_setting("k", &big),
        Err(StorageError::InvalidInput(_))
    ));
}

#[test]
fn history_titles_are_sanitized_before_they_are_stored() {
    let f = Fixture::new();
    let s = f.open();
    let p = ProfileKind::Default;
    s.record_visit(
        &p,
        &nav("https://a.example/1"),
        Some("  evil\u{202e}title\n\t here \u{0007}"),
        1,
    )
    .unwrap();
    s.record_visit(
        &p,
        &nav("https://a.example/2"),
        Some(&"w".repeat(10_000)),
        2,
    )
    .unwrap();
    s.record_visit(&p, &nav("https://a.example/3"), Some(" \u{200b}\n"), 3)
        .unwrap();
    let items = s.list_history(&p, 10, None).unwrap().items;
    assert_eq!(items[2].title.as_deref(), Some("eviltitle here"));
    assert_eq!(items[1].title.as_ref().unwrap().chars().count(), 256);
    assert_eq!(
        items[0].title, None,
        "a title that sanitizes to nothing is not stored"
    );
}

// ---- privacy ------------------------------------------------------------------------

#[test]
fn a_visit_in_a_private_session_is_never_written() {
    let f = Fixture::new();
    let s = f.open();
    let r = s
        .record_visit(
            &ProfileKind::Private,
            &nav("https://secret.example/x"),
            Some("Secret"),
            1,
        )
        .unwrap();
    assert_eq!(r, Persisted::SkippedPrivate);
    assert_eq!(count(&f.raw(), "history"), 0);
    assert!(s
        .list_history(&ProfileKind::Private, 10, None)
        .unwrap()
        .items
        .is_empty());
    // The same call for a normal profile does write, so the test above is meaningful.
    let r = s
        .record_visit(
            &ProfileKind::Default,
            &nav("https://open.example/"),
            None,
            2,
        )
        .unwrap();
    assert_eq!(r, Persisted::Stored);
    assert_eq!(count(&f.raw(), "history"), 1);
}

#[test]
fn a_permission_decision_in_a_private_session_is_never_written() {
    let f = Fixture::new();
    let s = f.open();
    let o = origin("https://cam.example");
    let r = s
        .set_permission(
            &ProfileKind::Private,
            &o,
            PermissionKind::Camera,
            PermissionDecision::Grant,
            1,
        )
        .unwrap();
    assert_eq!(r, Persisted::SkippedPrivate);
    assert_eq!(count(&f.raw(), "permissions"), 0);
    assert_eq!(
        s.get_permission(&ProfileKind::Private, &o, PermissionKind::Camera)
            .unwrap(),
        None
    );
}

#[test]
fn deleting_an_app_removes_its_icons_favorite_and_app_settings_but_nothing_else() {
    let f = Fixture::new();
    let s = f.open();
    let gone = rich_app();
    let kept = manual_app("https://kept.example/", "Kept");
    s.save_app(&gone, 1).unwrap();
    s.save_app(&kept, 2).unwrap();
    for app in [&gone, &kept] {
        s.add_favorite(app.id(), 3).unwrap();
        s.set_app_setting(app.id(), "notify", &SettingValue::Bool(true))
            .unwrap();
    }
    s.set_setting("ui.dark", &SettingValue::Bool(true)).unwrap();

    assert!(s.delete_app(gone.id()).unwrap());

    assert_eq!(s.get_app(gone.id()).unwrap(), None);
    assert!(!s.is_favorite(gone.id()).unwrap());
    assert_eq!(s.get_app_setting(gone.id(), "notify").unwrap(), None);
    assert!(s
        .list_favorites(10, None)
        .unwrap()
        .items
        .iter()
        .all(|a| a.id != *gone.id()));
    let raw = f.raw();
    assert_eq!(count(&raw, "app_icons"), 0);
    assert_eq!(count(&raw, "favorites"), 1);
    assert_eq!(count(&raw, "app_settings"), 1);
    assert!(s.is_favorite(kept.id()).unwrap());
    assert_eq!(
        s.get_setting("ui.dark").unwrap(),
        Some(SettingValue::Bool(true))
    );
    assert_fk_clean(&f);
}

#[test]
fn clearing_history_for_a_time_range_really_deletes_those_rows_and_only_those() {
    let f = Fixture::new();
    let s = f.open();
    s.create_profile(&ProfileId("work".into()), "Work", 0)
        .unwrap();
    let d = ProfileKind::Default;
    s.record_visit(&d, &nav("https://a.example/before"), None, 100)
        .unwrap();
    s.record_visit(&d, &nav("https://a.example/start-edge"), None, 150)
        .unwrap();
    s.record_visit(
        &d,
        &nav("https://shredded-marker.example/inside"),
        Some("shredded-title"),
        200,
    )
    .unwrap();
    s.record_visit(&d, &nav("https://a.example/end-edge"), None, 250)
        .unwrap();
    s.record_visit(&d, &nav("https://a.example/after"), None, 300)
        .unwrap();
    s.record_visit(
        &persistent("work"),
        &nav("https://a.example/work"),
        None,
        200,
    )
    .unwrap();

    let n = s
        .clear_history(
            &d,
            TimeRange {
                start: 150,
                end: 250,
            },
        )
        .unwrap();

    assert_eq!(n, 3);
    let times: Vec<_> = s
        .list_history(&d, 10, None)
        .unwrap()
        .items
        .iter()
        .map(|e| e.visited_at)
        .collect();
    assert_eq!(times, vec![300, 100]);
    assert_eq!(
        s.list_history(&persistent("work"), 10, None)
            .unwrap()
            .items
            .len(),
        1
    );
    assert_eq!(count(&f.raw(), "history"), 3);
    // Not merely unlinked: the bytes are gone from the main file and the log.
    drop(s);
    for path in [f.path(), f.dir.path().join("krypa.sqlite3-wal")] {
        if let Ok(bytes) = std::fs::read(&path) {
            let hay = String::from_utf8_lossy(&bytes);
            assert!(!hay.contains("shredded-marker"), "url survived in {path:?}");
            assert!(
                !hay.contains("shredded-title"),
                "title survived in {path:?}"
            );
        }
    }
}

#[test]
fn clearing_all_history_empties_the_profile_and_a_reversed_range_is_refused() {
    let f = Fixture::new();
    let s = f.open();
    let d = ProfileKind::Default;
    s.record_visit(&d, &nav("https://a.example/"), None, i64::MIN)
        .unwrap();
    s.record_visit(&d, &nav("https://a.example/"), None, i64::MAX - 1)
        .unwrap();
    assert!(matches!(
        s.clear_history(&d, TimeRange { start: 5, end: 1 }),
        Err(StorageError::InvalidInput(_))
    ));
    assert_eq!(s.clear_history(&d, TimeRange::all()).unwrap(), 2);
    assert_eq!(count(&f.raw(), "history"), 0);
    assert_eq!(
        s.clear_history(&ProfileKind::Private, TimeRange::all())
            .unwrap(),
        0
    );
}

#[test]
fn deleting_a_profile_removes_its_history_and_permissions_and_leaves_other_profiles_alone() {
    let f = Fixture::new();
    let s = f.open();
    s.create_profile(&ProfileId("work".into()), "Work", 0)
        .unwrap();
    s.create_profile(&ProfileId("play".into()), "Play", 0)
        .unwrap();
    let o = origin("https://cam.example");
    for p in [persistent("work"), persistent("play"), ProfileKind::Default] {
        s.record_visit(&p, &nav("https://a.example/"), None, 1)
            .unwrap();
        s.set_permission(&p, &o, PermissionKind::Camera, PermissionDecision::Grant, 1)
            .unwrap();
    }

    assert!(s.delete_profile(&ProfileId("work".into())).unwrap());

    let raw = f.raw();
    assert_eq!(count(&raw, "history"), 2);
    assert_eq!(count(&raw, "permissions"), 2);
    assert!(s
        .list_history(&persistent("work"), 10, None)
        .unwrap()
        .items
        .is_empty());
    assert_eq!(
        s.get_permission(&persistent("work"), &o, PermissionKind::Camera)
            .unwrap(),
        None
    );
    assert_eq!(
        s.list_history(&persistent("play"), 10, None)
            .unwrap()
            .items
            .len(),
        1
    );
    assert_eq!(
        s.list_history(&ProfileKind::Default, 10, None)
            .unwrap()
            .items
            .len(),
        1
    );
    assert_eq!(s.list_profiles(10, None).unwrap().items.len(), 1);
    assert!(matches!(
        s.record_visit(&persistent("work"), &nav("https://a.example/"), None, 2),
        Err(StorageError::UnknownProfile)
    ));
    assert_fk_clean(&f);
}

#[test]
fn the_default_profile_cannot_be_deleted_or_recreated_through_the_api() {
    let f = Fixture::new();
    let s = f.open();
    assert!(!s.delete_profile(&ProfileId("default".into())).unwrap());
    s.record_visit(&ProfileKind::Default, &nav("https://a.example/"), None, 1)
        .unwrap();
    assert_eq!(
        s.list_history(&ProfileKind::Default, 10, None)
            .unwrap()
            .items
            .len(),
        1
    );
}

#[test]
fn profile_ids_and_names_that_are_unsafe_are_rejected() {
    let f = Fixture::new();
    let s = f.open();
    for id in ["", "a b", "x/y", &"a".repeat(129), "tab\t"] {
        assert!(
            matches!(
                s.create_profile(&ProfileId(id.into()), "N", 0),
                Err(StorageError::InvalidInput(_))
            ),
            "id {id:?}"
        );
    }
    assert!(matches!(
        s.create_profile(&ProfileId("ok".into()), " \u{200b} ", 0),
        Err(StorageError::InvalidInput(_))
    ));
    s.create_profile(&ProfileId("ok".into()), "Fine", 0)
        .unwrap();
    assert!(matches!(
        s.create_profile(&ProfileId("ok".into()), "Again", 0),
        Err(StorageError::ProfileExists)
    ));
}

// ---- permission isolation ---------------------------------------------------------

#[test]
fn a_decision_for_one_origin_never_answers_for_another_origin_profile_or_kind() {
    let f = Fixture::new();
    let s = f.open();
    s.create_profile(&ProfileId("work".into()), "Work", 0)
        .unwrap();
    let d = ProfileKind::Default;
    let a = origin("https://a.example");
    s.set_permission(&d, &a, PermissionKind::Camera, PermissionDecision::Grant, 1)
        .unwrap();

    assert_eq!(
        s.get_permission(&d, &a, PermissionKind::Camera).unwrap(),
        Some(PermissionDecision::Grant)
    );
    // Same origin reached through a different path.
    assert_eq!(
        s.get_permission(
            &d,
            &origin("https://a.example/other/page?x=1"),
            PermissionKind::Camera
        )
        .unwrap(),
        Some(PermissionDecision::Grant)
    );
    for other in [
        "http://a.example",
        "https://a.example:8443",
        "https://sub.a.example",
        "https://a.example.evil.test",
        "https://b.example",
    ] {
        assert_eq!(
            s.get_permission(&d, &origin(other), PermissionKind::Camera)
                .unwrap(),
            None,
            "{other}"
        );
    }
    assert_eq!(
        s.get_permission(&persistent("work"), &a, PermissionKind::Camera)
            .unwrap(),
        None
    );
    assert_eq!(
        s.get_permission(&ProfileKind::Private, &a, PermissionKind::Camera)
            .unwrap(),
        None
    );
    for kind in [
        PermissionKind::Microphone,
        PermissionKind::CameraAndMicrophone,
        PermissionKind::Geolocation,
        PermissionKind::MotionAndOrientation,
    ] {
        assert_eq!(s.get_permission(&d, &a, kind).unwrap(), None);
    }
}

#[test]
fn a_later_decision_replaces_the_earlier_one_and_can_be_cleared() {
    let f = Fixture::new();
    let s = f.open();
    let d = ProfileKind::Default;
    let a = origin("https://a.example");
    s.set_permission(
        &d,
        &a,
        PermissionKind::Geolocation,
        PermissionDecision::Grant,
        1,
    )
    .unwrap();
    s.set_permission(
        &d,
        &a,
        PermissionKind::Geolocation,
        PermissionDecision::Deny,
        2,
    )
    .unwrap();
    assert_eq!(
        s.get_permission(&d, &a, PermissionKind::Geolocation)
            .unwrap(),
        Some(PermissionDecision::Deny)
    );
    assert_eq!(count(&f.raw(), "permissions"), 1);
    assert!(s
        .clear_permission(&d, &a, PermissionKind::Geolocation)
        .unwrap());
    assert_eq!(
        s.get_permission(&d, &a, PermissionKind::Geolocation)
            .unwrap(),
        None
    );
}

#[test]
fn listed_permissions_round_trip_origins_including_ports_ipv6_and_idn() {
    let f = Fixture::new();
    let s = f.open();
    let d = ProfileKind::Default;
    let hosts = [
        "https://a.example",
        "http://a.example:8080",
        "https://[::1]:3000",
        "http://192.168.0.1",
        "https://bücher.example",
    ];
    for h in hosts {
        s.set_permission(
            &d,
            &origin(h),
            PermissionKind::Camera,
            PermissionDecision::Deny,
            1,
        )
        .unwrap();
    }
    let listed = s.list_permissions(&d, 50, None).unwrap().items;
    assert_eq!(listed.len(), hosts.len());
    for h in hosts {
        assert!(listed.iter().any(|r| r.origin == origin(h)), "{h}");
    }
}

// ---- unknown references ------------------------------------------------------------

#[test]
fn referencing_a_missing_app_or_profile_is_a_typed_error() {
    let f = Fixture::new();
    let s = f.open();
    let ghost = url("https://ghost.example/");
    assert!(matches!(
        s.add_favorite(&ghost, 1),
        Err(StorageError::UnknownApp)
    ));
    assert!(matches!(
        s.set_app_setting(&ghost, "k", &SettingValue::Bool(true)),
        Err(StorageError::UnknownApp)
    ));
    assert!(matches!(
        s.set_permission(
            &persistent("nope"),
            &origin("https://a.example"),
            PermissionKind::Camera,
            PermissionDecision::Grant,
            1
        ),
        Err(StorageError::UnknownProfile)
    ));
}

// ---- pagination --------------------------------------------------------------------

#[test]
fn history_pages_are_newest_first_without_gaps_or_overlap() {
    let f = Fixture::new();
    let s = f.open();
    let d = ProfileKind::Default;
    for i in 0..5 {
        // Two visits share a timestamp to prove the id tiebreak.
        let at = if i < 2 { 10 } else { 10 + i };
        s.record_visit(&d, &nav(&format!("https://a.example/{i}")), None, at)
            .unwrap();
    }
    let mut seen = Vec::new();
    let mut cursor = None;
    let mut pages = 0;
    loop {
        let page = s.list_history(&d, 2, cursor.as_ref()).unwrap();
        assert!(page.items.len() <= 2);
        seen.extend(page.items.iter().map(|e| e.url.to_string()));
        pages += 1;
        match page.next {
            Some(c) => cursor = Some(c),
            None => break,
        }
    }
    assert_eq!(pages, 3);
    assert_eq!(
        seen,
        ["4", "3", "2", "1", "0"].map(|i| format!("https://a.example/{i}"))
    );
}

#[test]
fn app_and_favorite_lists_are_paginated_and_return_only_list_fields() {
    let f = Fixture::new();
    let s = f.open();
    let names = ["Delta", "alpha", "Charlie", "Bravo", "Echo"];
    let apps: Vec<_> = names
        .iter()
        .map(|n| manual_app(&format!("https://{}.example/", n.to_lowercase()), n))
        .collect();
    for (i, a) in apps.iter().enumerate() {
        s.save_app(a, i as i64).unwrap();
        s.add_favorite(a.id(), i as i64).unwrap();
    }
    let mut got = Vec::new();
    let mut cursor = None;
    loop {
        let page = s.list_apps(2, cursor.as_ref()).unwrap();
        got.extend(page.items.iter().map(|a| a.name.clone()));
        match page.next {
            Some(c) => cursor = Some(c),
            None => break,
        }
    }
    assert_eq!(got, ["Bravo", "Charlie", "Delta", "Echo", "alpha"]);

    let fav = s.list_favorites(3, None).unwrap();
    assert_eq!(
        fav.items
            .iter()
            .map(|a| a.name.as_str())
            .collect::<Vec<_>>(),
        ["Echo", "Bravo", "Charlie"]
    );
    let rest = s.list_favorites(3, fav.next.as_ref()).unwrap();
    assert_eq!(
        rest.items
            .iter()
            .map(|a| a.name.as_str())
            .collect::<Vec<_>>(),
        ["alpha", "Delta"]
    );
    assert!(rest.next.is_none());
    assert_eq!(
        s.list_apps(0, None).unwrap().items.len(),
        1,
        "limit 0 is raised to 1"
    );
    assert_eq!(
        s.list_apps(usize::MAX, None).unwrap().items.len(),
        5,
        "limit is capped, not rejected"
    );
}

#[test]
fn settings_and_profile_lists_are_paginated() {
    let f = Fixture::new();
    let s = f.open();
    for k in ["a", "b", "c"] {
        s.set_setting(k, &SettingValue::Int(1)).unwrap();
    }
    let p1 = s.list_settings(2, None).unwrap();
    assert_eq!(
        p1.items.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>(),
        ["a", "b"]
    );
    let p2 = s.list_settings(2, p1.next.as_deref()).unwrap();
    assert_eq!(p2.items.len(), 1);
    assert!(p2.next.is_none());
    for id in ["p1", "p2", "p3"] {
        s.create_profile(&ProfileId(id.into()), id, 0).unwrap();
    }
    let q1 = s.list_profiles(2, None).unwrap();
    let q2 = s.list_profiles(2, q1.next).unwrap();
    assert_eq!((q1.items.len(), q2.items.len()), (2, 1));
    assert!(q2.next.is_none());
}

// ---- corrupted rows ----------------------------------------------------------------

fn assert_corrupt<T: std::fmt::Debug>(r: Result<T, StorageError>, table: &str) {
    match r {
        Err(e @ StorageError::CorruptRow { .. }) => {
            let StorageError::CorruptRow { table: t, .. } = &e else {
                unreachable!()
            };
            assert_eq!(*t, table);
            let shown = format!("{e} {e:?}");
            assert!(
                !shown.contains("javascript") && !shown.contains("evil"),
                "error leaks data: {shown}"
            );
        }
        other => panic!("expected CorruptRow in {table}, got {other:?}"),
    }
}

#[test]
fn a_history_row_with_a_hostile_url_is_a_typed_error_not_a_panic_or_a_skip() {
    let f = Fixture::new();
    let s = f.open();
    let d = ProfileKind::Default;
    for i in 0..3 {
        s.record_visit(&d, &nav(&format!("https://a.example/{i}")), None, i)
            .unwrap();
    }
    f.raw()
        .execute(
            "UPDATE history SET url = 'javascript:alert(1)' WHERE visited_at = 1",
            [],
        )
        .unwrap();
    // The middle row is bad: the call must fail loudly instead of returning two entries.
    assert_corrupt(s.list_history(&d, 10, None), "history");
}

#[test]
fn history_rows_with_credentials_garbage_or_control_characters_are_rejected() {
    for (column, bad) in [
        ("url", "https://user:pw@evil.example/"),
        ("url", "not a url"),
        ("url", ""),
        ("title", "bad\u{0007}title"),
        ("title", ""),
    ] {
        let f = Fixture::new();
        let s = f.open();
        s.record_visit(
            &ProfileKind::Default,
            &nav("https://a.example/"),
            Some("ok"),
            1,
        )
        .unwrap();
        f.raw()
            .execute(&format!("UPDATE history SET {column} = ?1"), [bad])
            .unwrap();
        assert_corrupt(s.list_history(&ProfileKind::Default, 10, None), "history");
    }
}

#[test]
fn an_app_row_that_no_longer_matches_its_validated_form_is_rejected() {
    for (column, bad) in [
        ("scope", "https://evil.example/"),
        ("start_url", "javascript:alert(1)"),
        ("name", "bad\u{202e}name"),
        ("name", ""),
        ("theme_color", "not-a-colour"),
        ("id", "file:///etc/passwd"),
    ] {
        let f = Fixture::new();
        let s = f.open();
        let app = rich_app();
        s.save_app(&app, 1).unwrap();
        f.raw()
            .execute(&format!("UPDATE installed_apps SET {column} = ?1"), [bad])
            .unwrap();
        // The list returns (and therefore validates) only id, name and start_url.
        if matches!(column, "id" | "name" | "start_url") {
            assert_corrupt(s.list_apps(10, None), "installed_apps");
        }
        // get_app looks up by id, so only rows whose id is intact are reachable that way.
        if column != "id" {
            assert_corrupt(s.get_app(app.id()), "installed_apps");
        }
    }
}

#[test]
fn an_app_whose_icon_row_was_tampered_with_is_rejected() {
    let f = Fixture::new();
    let s = f.open();
    let app = rich_app();
    s.save_app(&app, 1).unwrap();
    f.raw()
        .execute(
            "UPDATE app_icons SET src = 'javascript:alert(1)' WHERE position = 0",
            [],
        )
        .unwrap();
    assert_corrupt(s.get_app(app.id()), "installed_apps");
}

#[test]
fn a_permission_row_with_an_unparseable_origin_kind_or_decision_is_rejected() {
    for (column, bad) in [
        ("origin", "garbage"),
        ("origin", "https://a.example:443/evil"),
        ("origin", "ftp://a.example:21"),
        ("origin", "https://user@a.example:443"),
        ("kind", "teleport"),
        ("decision", "maybe"),
    ] {
        let f = Fixture::new();
        let s = f.open();
        let o = origin("https://a.example");
        s.set_permission(
            &ProfileKind::Default,
            &o,
            PermissionKind::Camera,
            PermissionDecision::Grant,
            1,
        )
        .unwrap();
        f.raw()
            .execute(&format!("UPDATE permissions SET {column} = ?1"), [bad])
            .unwrap();
        assert_corrupt(
            s.list_permissions(&ProfileKind::Default, 10, None),
            "permissions",
        );
        if column == "decision" {
            assert_corrupt(
                s.get_permission(&ProfileKind::Default, &o, PermissionKind::Camera),
                "permissions",
            );
        }
    }
}

#[test]
fn a_setting_row_whose_type_and_value_disagree_is_rejected() {
    let f = Fixture::new();
    let s = f.open();
    s.set_setting("flag", &SettingValue::Bool(true)).unwrap();
    s.set_setting("n", &SettingValue::Int(1)).unwrap();
    s.set_setting("t", &SettingValue::Text("x".into())).unwrap();
    let raw = f.raw();
    raw.execute("UPDATE settings SET int_value = 7 WHERE key = 'flag'", [])
        .unwrap();
    raw.execute("UPDATE settings SET int_value = NULL WHERE key = 'n'", [])
        .unwrap();
    raw.execute("UPDATE settings SET text_value = NULL WHERE key = 't'", [])
        .unwrap();
    for key in ["flag", "n", "t"] {
        assert_corrupt(s.get_setting(key), "settings");
    }
    assert_corrupt(s.list_settings(10, None), "settings");
}

#[test]
fn a_profile_row_with_a_hostile_id_or_name_is_rejected() {
    for (column, bad) in [("profile_id", "has space"), ("name", "bad\u{0007}name")] {
        let f = Fixture::new();
        let s = f.open();
        s.create_profile(&ProfileId("work".into()), "Work", 0)
            .unwrap();
        f.raw()
            .execute(
                &format!("UPDATE profiles SET {column} = ?1 WHERE kind = 'persistent'"),
                [bad],
            )
            .unwrap();
        assert_corrupt(s.list_profiles(10, None), "profiles");
    }
}

#[test]
fn a_file_that_is_not_a_database_is_a_typed_error() {
    let f = Fixture::new();
    std::fs::write(
        f.path(),
        b"this is definitely not sqlite, just some text padding",
    )
    .unwrap();
    let err = Storage::open(&f.path()).unwrap_err();
    assert!(matches!(err, StorageError::Sqlite(_)), "{err:?}");
}

#[test]
fn foreign_key_check_is_clean_after_a_mixed_workload_with_cascades() {
    let f = Fixture::new();
    let s = f.open();
    let app = rich_app();
    s.save_app(&app, 1).unwrap();
    s.add_favorite(app.id(), 1).unwrap();
    s.set_app_setting(app.id(), "k", &SettingValue::Bool(false))
        .unwrap();
    s.create_profile(&ProfileId("p".into()), "P", 1).unwrap();
    s.record_visit(&persistent("p"), &nav("https://a.example/"), None, 1)
        .unwrap();
    s.set_permission(
        &persistent("p"),
        &origin("https://a.example"),
        PermissionKind::Microphone,
        PermissionDecision::Deny,
        1,
    )
    .unwrap();
    s.delete_app(app.id()).unwrap();
    s.delete_profile(&ProfileId("p".into())).unwrap();
    assert_fk_clean(&f);
    let raw = f.raw();
    for t in [
        "installed_apps",
        "app_icons",
        "favorites",
        "app_settings",
        "history",
        "permissions",
    ] {
        assert_eq!(count(&raw, t), 0, "{t}");
    }
}

#[test]
fn every_foreign_key_and_filter_column_has_an_index() {
    let f = Fixture::new();
    drop(f.open());
    let conn = f.raw();
    // Each (table, leading column) pair is a FK or a filter/sort column used by a query.
    let needed = [
        ("app_icons", "app_id"),
        ("favorites", "app_id"),
        ("favorites", "created_at"),
        ("app_settings", "app_id"),
        ("installed_apps", "name"),
        ("history", "profile_ref"),
        ("history", "visited_at"),
        ("permissions", "profile_ref"),
    ];
    for (table, column) in needed {
        let mut found = false;
        let mut idx = conn
            .prepare(&format!("PRAGMA index_list({table})"))
            .unwrap();
        let names: Vec<String> = idx
            .query_map([], |r| r.get::<_, String>(1))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        for name in names {
            let mut info = conn.prepare(&format!("PRAGMA index_info({name})")).unwrap();
            let cols: Vec<String> = info
                .query_map([], |r| r.get::<_, String>(2))
                .unwrap()
                .map(Result::unwrap)
                .collect();
            // history is filtered by profile_ref then sorted by visited_at: both appear.
            found |= cols.first().is_some_and(|c| c == column)
                || (table == "history" && cols.iter().any(|c| c == column));
        }
        assert!(found, "no index covers {table}.{column}");
    }
}

// ---- installed apps survive a restart ------------------------------------------------

#[test]
fn an_installed_app_is_equal_after_save_close_reopen_and_load() {
    // Why: without this an installed app vanishes (or silently changes) on restart.
    let f = Fixture::new();
    let apps = [
        rich_app(),
        manual_app("https://plain.example/dir/page?q=1", "Plain"),
    ];
    {
        let s = f.open();
        for (i, a) in apps.iter().enumerate() {
            s.save_app(a, i as i64).unwrap();
        }
    }
    let s = f.open();
    for a in &apps {
        assert_eq!(&s.get_app(a.id()).unwrap().unwrap(), a);
    }
    let page = s.list_installed_apps(10, None).unwrap();
    assert!(page.errors.is_empty() && page.next.is_none());
    assert_eq!(page.apps.len(), apps.len());
    for a in &apps {
        assert!(page.apps.contains(a), "{} missing from list", a.name());
    }
    // Every persisted field, spelled out so a dropped column cannot hide behind `==`.
    let loaded = s.get_app(apps[0].id()).unwrap().unwrap();
    assert_eq!(loaded.theme_color(), apps[0].theme_color());
    assert_eq!(loaded.background_color(), apps[0].background_color());
    assert_eq!(loaded.icons(), apps[0].icons());
    assert_eq!(loaded.scope(), apps[0].scope());
    assert_eq!(loaded.display(), apps[0].display());
    assert_eq!(loaded.icons().len(), 2, "the http icon was never installed");
}

#[test]
fn a_loaded_app_keeps_its_scope_decisions() {
    let f = Fixture::new();
    let s = f.open();
    let app = rich_app();
    s.save_app(&app, 1).unwrap();
    let back = s.get_app(app.id()).unwrap().unwrap();
    for u in [
        "https://notes.example/notes/x",
        "https://notes.example/notes",
        "https://notes.example/other",
        "https://evil.example/notes/x",
        "http://notes.example/notes/x",
    ] {
        assert_eq!(back.is_in_scope(&url(u)), app.is_in_scope(&url(u)), "{u}");
    }
}

#[test]
fn tampered_scope_origin_or_start_never_load_as_an_app() {
    // The scope is the boundary between "inside the app" and "external".
    for (column, bad) in [
        ("scope", "https://evil.example/"),
        ("scope", "http://notes.example/notes/"),
        ("scope", "https://notes.example:444/notes/"),
        ("scope", "https://notes.example/elsewhere/"),
        ("scope", "ftp://notes.example/notes/"),
        ("scope", "not a url"),
        ("start_url", "https://evil.example/notes/home"),
        ("start_url", "http://notes.example/notes/home"),
        ("start_url", "https://NOTES.example/notes/home"),
        ("id", "https://evil.example/notes"),
    ] {
        let f = Fixture::new();
        let s = f.open();
        let app = rich_app();
        s.save_app(&app, 1).unwrap();
        f.raw()
            .execute(&format!("UPDATE installed_apps SET {column} = ?1"), [bad])
            .unwrap();
        let page = s.list_installed_apps(10, None).unwrap();
        assert!(page.apps.is_empty(), "{column}={bad} loaded");
        assert_eq!(page.errors.len(), 1, "{column}={bad}");
        assert_corrupt(
            Err::<(), _>(page.errors.into_iter().next().unwrap()),
            "installed_apps",
        );
        if column != "id" {
            assert_corrupt(s.get_app(app.id()), "installed_apps");
        }
    }
}

#[test]
fn icon_and_colour_text_that_would_be_rewritten_on_the_way_back_is_corrupt() {
    for (sql, what) in [
        (
            "UPDATE app_icons SET sizes = '192x192  any' WHERE position = 0",
            "double space",
        ),
        (
            "UPDATE app_icons SET sizes = '0192x192' WHERE position = 0",
            "leading zero",
        ),
        (
            "UPDATE app_icons SET sizes = '0x0' WHERE position = 0",
            "zero size",
        ),
        (
            "UPDATE app_icons SET purposes = '' WHERE position = 0",
            "no purpose",
        ),
        (
            "UPDATE app_icons SET purposes = 'maskable any' WHERE position = 0",
            "order",
        ),
        (
            "UPDATE app_icons SET src = 'http://notes.example/i.png' WHERE position = 0",
            "http icon",
        ),
        (
            "UPDATE installed_apps SET theme_color = '#FFF'",
            "short colour",
        ),
        (
            "UPDATE installed_apps SET theme_color = '#AABBCCDD'",
            "upper-case colour",
        ),
    ] {
        let f = Fixture::new();
        let s = f.open();
        let app = rich_app();
        s.save_app(&app, 1).unwrap();
        f.raw().execute(sql, []).unwrap();
        assert_corrupt(s.get_app(app.id()), "installed_apps");
        let page = s.list_installed_apps(10, None).unwrap();
        assert_eq!(page.errors.len(), 1, "{what}");
    }
}

#[test]
fn one_bad_row_is_reported_without_hiding_or_failing_the_good_ones() {
    // Why: one corrupt app must not make every installed app disappear on restart, and
    // must not vanish silently either.
    let f = Fixture::new();
    let s = f.open();
    let a = manual_app("https://a.example/", "Alpha");
    let b = manual_app("https://b.example/", "Bravo");
    let c = manual_app("https://c.example/", "Charlie");
    for (i, app) in [&a, &b, &c].into_iter().enumerate() {
        s.save_app(app, i as i64).unwrap();
    }
    f.raw()
        .execute(
            "UPDATE installed_apps SET scope = 'https://evil.example/' WHERE name = 'Bravo'",
            [],
        )
        .unwrap();
    let page = s.list_installed_apps(10, None).unwrap();
    assert_eq!(page.apps, vec![a.clone(), c.clone()]);
    assert_eq!(page.errors.len(), 1);
    assert!(matches!(
        &page.errors[0],
        StorageError::CorruptRow {
            table: "installed_apps",
            row: Some(_),
            field: "scope"
        }
    ));
    // Paging walks past the bad row too.
    let first = s.list_installed_apps(2, None).unwrap();
    assert_eq!((first.apps.len(), first.errors.len()), (1, 1));
    let second = s.list_installed_apps(2, first.next.as_ref()).unwrap();
    assert_eq!(second.apps, vec![c]);
    assert!(second.next.is_none());
}

#[test]
fn a_stored_plain_http_app_is_corrupt_and_the_https_apps_still_load() {
    // Why: installed apps are https-only. A row written before that rule, or planted by
    // hand, is internally consistent but must never become an app, and it must not hide
    // the good ones next to it.
    let f = Fixture::new();
    let s = f.open();
    let a = manual_app("https://a.example/", "Alpha");
    let b = manual_app("https://b.example/", "Bravo");
    let c = manual_app("https://c.example/", "Charlie");
    for (i, app) in [&a, &b, &c].into_iter().enumerate() {
        s.save_app(app, i as i64).unwrap();
    }
    f.raw()
        .execute(
            "UPDATE installed_apps SET id = 'http://b.example/', start_url = 'http://b.example/',
                    scope = 'http://b.example/' WHERE name = 'Bravo'",
            [],
        )
        .unwrap();
    let page = s.list_installed_apps(10, None).unwrap();
    assert_eq!(page.apps, vec![a, c]);
    assert_eq!(page.errors.len(), 1);
    assert!(matches!(
        &page.errors[0],
        StorageError::CorruptRow {
            table: "installed_apps",
            row: Some(_),
            field: "id"
        }
    ));
    assert_corrupt(s.get_app(&url("http://b.example/")), "installed_apps");
}

#[test]
fn an_empty_database_lists_no_installed_apps_and_no_errors() {
    let f = Fixture::new();
    let page = f.open().list_installed_apps(10, None).unwrap();
    assert!(page.apps.is_empty() && page.errors.is_empty() && page.next.is_none());
}
