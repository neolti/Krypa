//! The wiring a platform shell will also need: UI intents in, `SessionManager` calls,
//! engine commands out, engine events back in. No egui here, so it is testable headless.

use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use eframe::egui::ColorImage;
use krypa_apps::InstalledApp;
use krypa_core::{
    CoreError, ManagerConfig, MemoryPressure, Outcome, SessionManager, SessionSpec, SessionState,
};
use krypa_engine_api::{
    EngineCapabilities, EngineCommand, EngineEvent, IconFetchRequest, IconFetchResult,
    PermissionKind, ProfileKind, SessionId,
};
use krypa_navigation::{NavigationUrl, SearchConfig};
use krypa_storage::{Storage, MAX_PAGE};
use krypa_ui::app_icons::{decode_icon, IconDecodeError, MAX_ICON_BYTES};
use krypa_ui::{AppState, Intent, RecentEntry, Screen};
use url::Url;

use crate::engine::{fetch_icon, FakeEngine};

/// A finished icon job: the URL asked for and the decoded image or why there is none.
type IconDone = (Url, Result<ColorImage, IconDecodeError>);

/// Live web views kept at once; small on purpose so eviction is easy to see.
const LIVE_VIEW_BUDGET: usize = 3;
const MAX_RECENTS: usize = 8;

/// Manager, fake engine and UI state, wired together.
pub struct Core {
    /// The real session manager.
    pub manager: SessionManager,
    /// The fake engine.
    pub engine: FakeEngine,
    /// What the UI draws.
    pub state: AppState,
    /// Last thing worth telling the developer (errors from the core, mostly).
    pub status: String,
    /// What went wrong while loading installed apps at startup (one line per bad row);
    /// the shell prints these, they are also summarised in `status`.
    pub startup_problems: Vec<String>,
    search: SearchConfig,
    storage: Option<Storage>,
    icon_tx: Sender<IconDone>,
    icon_rx: Receiver<IconDone>,
    icons_in_flight: usize,
}

impl Core {
    /// A core with a three-view budget and every capability the fake engine pretends to have.
    ///
    /// # Errors
    /// Only if the budget constant were zero.
    pub fn new() -> Result<Self, CoreError> {
        let capabilities = EngineCapabilities {
            service_workers: true,
            passkeys: true,
            add_to_home_screen: true,
            persistent_profiles: true,
            private_sessions: true,
            content_blocking: false,
            view_suspend: false,
            manifest_fetch: true,
        };
        let config = ManagerConfig::new(LIVE_VIEW_BUDGET)?;
        let manager = SessionManager::new(config, capabilities);
        let mut state = AppState::new();
        state.sync_sessions(&manager);
        let (icon_tx, icon_rx) = channel();
        Ok(Self {
            manager,
            engine: FakeEngine::new(),
            state,
            status: String::new(),
            startup_problems: Vec::new(),
            search: SearchConfig::default(),
            storage: None,
            icon_tx,
            icon_rx,
            icons_in_flight: 0,
        })
    }

    /// Uses `storage` for installed apps from now on and loads the apps already in it, so
    /// installs survive a restart. A row that does not validate is not dropped silently: it
    /// is listed in `startup_problems` and counted in `status`, and the others load.
    pub fn attach_storage(&mut self, storage: Storage) {
        let mut apps = Vec::new();
        let mut after = None;
        let mut corrupt = 0;
        loop {
            match storage.list_installed_apps(MAX_PAGE, after.as_ref()) {
                Ok(page) => {
                    apps.extend(page.apps);
                    corrupt += page.errors.len();
                    self.startup_problems
                        .extend(page.errors.iter().map(ToString::to_string));
                    after = page.next;
                    if after.is_none() {
                        break;
                    }
                }
                Err(e) => {
                    self.startup_problems
                        .push(format!("could not read the installed apps: {e}"));
                    break;
                }
            }
        }
        if !self.startup_problems.is_empty() {
            self.status = format!(
                "{corrupt} stored app(s) could not be loaded, {} problem(s) in total",
                self.startup_problems.len()
            );
        }
        self.state.installed_apps = apps;
        self.storage = Some(storage);
    }

    /// Whether an icon is still being fetched (the caller keeps repainting while true).
    #[must_use]
    pub fn icons_pending(&self) -> bool {
        self.icons_in_flight > 0
    }

    /// Delivers engine events that are due and finished icon jobs.
    pub fn pump(&mut self, now: Instant) {
        for event in self.engine.due(now) {
            self.deliver(event, now);
        }
        while let Ok((url, result)) = self.icon_rx.try_recv() {
            self.icons_in_flight = self.icons_in_flight.saturating_sub(1);
            self.state.icons.finish(&url, result);
            if let Some(why) = self.state.icons.failure(&url) {
                self.status = format!("an app icon is not shown: {why:?}");
            }
        }
    }

    /// Feeds one engine event to the manager and applies the result.
    pub fn deliver(&mut self, event: EngineEvent, now: Instant) {
        match self.manager.handle_event(event) {
            Ok(outcome) => self.apply(outcome, now),
            Err(e) => self.status = format!("core refused an engine event: {e}"),
        }
        self.state.sync_sessions(&self.manager);
    }

    /// Executes what the UI asked for.
    pub fn handle_intents(&mut self, intents: Vec<Intent>, now: Instant) {
        for intent in intents {
            self.handle_intent(intent, now);
        }
        self.state.sync_sessions(&self.manager);
    }

    fn handle_intent(&mut self, intent: Intent, now: Instant) {
        let result = match intent {
            Intent::Navigate(url) => {
                self.remember(&url);
                self.open(url.as_url().clone(), None, now)
            }
            Intent::Search(query) => match self.search.search_url(&query) {
                Ok(url) => self.open(url.as_url().clone(), None, now),
                Err(e) => {
                    self.status = format!("search failed: {e}");
                    Ok(())
                }
            },
            Intent::OpenExternal(link) => {
                self.status = format!("would ask before opening a {} link", link.scheme());
                Ok(())
            }
            Intent::OpenApp(id) => self.open_app(&id, now),
            Intent::GoBack(id) => self.call(id, now, SessionManager::go_back),
            Intent::GoForward(id) => self.call(id, now, SessionManager::go_forward),
            Intent::Reload(id) => self.call(id, now, SessionManager::reload),
            Intent::Stop(id) => self.call(id, now, SessionManager::stop),
            Intent::SwitchTo(id) => self.call(id, now, SessionManager::foreground),
            Intent::CloseSession(id) => self.call(id, now, SessionManager::close_session),
            Intent::AddToKrypa(id) => self.call(id, now, SessionManager::request_install),
            Intent::ConfirmInstall(app) => {
                self.install(*app);
                Ok(())
            }
            Intent::RespondExternalOpen { session, allow } => self
                .manager
                .respond_external_open(session, allow)
                .map(|outcome| self.apply(outcome, now)),
            Intent::FetchIcon(url) => {
                self.fetch_icon(url);
                Ok(())
            }
            Intent::RespondPermission {
                session,
                request,
                decision,
            } => match self.manager.respond_permission(session, request, decision) {
                Ok(outcome) => {
                    self.apply(outcome, now);
                    Ok(())
                }
                Err(e) => Err(e),
            },
            Intent::SettingsChanged => Ok(()),
        };
        if let Err(e) = result {
            self.status = format!("core refused: {e}");
        }
    }

    fn call(
        &mut self,
        id: SessionId,
        now: Instant,
        f: fn(&mut SessionManager, SessionId) -> Result<Outcome, CoreError>,
    ) -> Result<(), CoreError> {
        let outcome = f(&mut self.manager, id)?;
        self.apply(outcome, now);
        Ok(())
    }

    /// Opens `url` in a new foreground session.
    pub fn open(
        &mut self,
        url: Url,
        app_scope: Option<Url>,
        now: Instant,
    ) -> Result<(), CoreError> {
        let id = self.manager.create_session(SessionSpec {
            profile: ProfileKind::Default,
            url: Some(url),
            app_scope,
        })?;
        let outcome = self.manager.foreground(id)?;
        self.apply(outcome, now);
        self.state.screen = Screen::Browser;
        Ok(())
    }

    fn open_app(&mut self, id: &Url, now: Instant) -> Result<(), CoreError> {
        let Some(app) = self.state.installed_apps.iter().find(|a| a.id() == id) else {
            self.status = "unknown app".to_owned();
            return Ok(());
        };
        let (start, scope) = (app.start_url().clone(), app.scope().clone());
        self.open(start, Some(scope), now)
    }

    /// Saves a confirmed app (in storage when there is one, then in the list the launcher
    /// shows). Installing an app that exists replaces it, as `Storage::save_app` does.
    fn install(&mut self, app: InstalledApp) {
        if let Some(storage) = &self.storage {
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX));
            if let Err(e) = storage.save_app(&app, now) {
                self.status = format!("could not save the app: {e}");
                return;
            }
        }
        match self
            .state
            .installed_apps
            .iter_mut()
            .find(|a| a.id() == app.id())
        {
            Some(existing) => *existing = app,
            None => self.state.installed_apps.push(app),
        }
    }

    /// The host side of an icon fetch: no network in the harness, the fake host answers.
    /// Fetching and decoding run on a worker thread so the UI thread never waits on them.
    fn fetch_icon(&mut self, url: Url) {
        if url.scheme() != "https" {
            self.state
                .icons
                .finish(&url, Err(IconDecodeError::Unavailable));
            return;
        }
        let tx = self.icon_tx.clone();
        self.icons_in_flight += 1;
        std::thread::spawn(move || {
            let request = IconFetchRequest {
                url: url.clone(),
                max_bytes: u32::try_from(MAX_ICON_BYTES).unwrap_or(u32::MAX),
            };
            let result = match fetch_icon(&request) {
                IconFetchResult::Fetched(bytes) => decode_icon(&bytes),
                IconFetchResult::Failed => Err(IconDecodeError::Unavailable),
            };
            // The receiver is gone only when the harness is closing.
            let _ = tx.send((url, result));
        });
    }

    /// Harness control: the foreground page asks the system to open `url`.
    pub fn page_requests_external(&mut self, url: &str, user_gesture: bool, now: Instant) {
        let (Some(session), Ok(url)) = (self.manager.foreground_session(), Url::parse(url)) else {
            return;
        };
        self.deliver(
            EngineEvent::ExternalSchemeRequested {
                session,
                url,
                user_gesture,
            },
            now,
        );
    }

    fn remember(&mut self, url: &NavigationUrl) {
        let host = url.as_url().host_str().unwrap_or_default().to_owned();
        self.state.recents.retain(|r| r.url != *url);
        self.state.recents.insert(
            0,
            RecentEntry {
                title: host,
                url: url.clone(),
            },
        );
        self.state.recents.truncate(MAX_RECENTS);
    }

    /// Applies an outcome: commands to the engine, notices to the UI.
    fn apply(&mut self, outcome: Outcome, now: Instant) {
        for command in outcome.commands {
            if let EngineCommand::OpenExternal { url, .. } = &command {
                self.status = format!("handed a {} link to the OS (logged)", url.scheme());
            }
            self.engine.execute(command, now);
        }
        for notice in &outcome.notices {
            self.state.apply_notice(notice);
        }
    }

    /// Harness control: the foreground page's content process dies.
    pub fn crash_foreground(&mut self, now: Instant) {
        if let Some(event) = self
            .manager
            .foreground_session()
            .and_then(|id| self.engine.crash(id))
        {
            self.deliver(event, now);
        }
    }

    /// Harness control: the OS reports memory pressure.
    pub fn memory_pressure(&mut self, level: MemoryPressure, now: Instant) {
        match self.manager.on_memory_pressure(level) {
            Ok(outcome) => self.apply(outcome, now),
            Err(e) => self.status = format!("memory pressure failed: {e}"),
        }
        self.state.sync_sessions(&self.manager);
    }

    /// Harness control: a page in the foreground session asks for a permission.
    pub fn request_permission(&mut self, kind: PermissionKind, now: Instant) {
        let Some(info) = self
            .manager
            .foreground_session()
            .and_then(|id| self.manager.info(id).ok())
        else {
            return;
        };
        let Some(origin) = info.current_url else {
            return;
        };
        let event = self.engine.permission_request(info.id, kind, origin);
        self.deliver(event, now);
    }

    /// Harness control: pause every background view (the manager marks them `Suspended`).
    pub fn suspend_background(&mut self, now: Instant) {
        let background: Vec<SessionId> = self
            .manager
            .sessions()
            .into_iter()
            .filter(|s| s.state == SessionState::Background)
            .map(|s| s.id)
            .collect();
        for id in background {
            if let Ok(outcome) = self.manager.suspend(id) {
                self.apply(outcome, now);
            }
        }
        self.state.sync_sessions(&self.manager);
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use krypa_core::{InstallSource, ManualReason};
    use krypa_engine_api::{ManifestResult, PermissionDecision};

    use super::*;

    fn nav(s: &str) -> Intent {
        Intent::Navigate(NavigationUrl::parse(s).unwrap())
    }

    fn run(core: &mut Core, from: Instant) {
        core.pump(from + Duration::from_secs(5));
    }

    /// Why: the UI promises intents; the wiring must turn them into the lifecycle the
    /// manager defines, not a parallel one.
    #[test]
    fn navigating_from_the_launcher_opens_and_commits_a_foreground_session() {
        let mut core = Core::new().unwrap();
        let t0 = Instant::now();
        core.handle_intents(vec![nav("https://ok.example/")], t0);
        assert_eq!(core.state.screen, Screen::Browser);
        run(&mut core, t0);
        let info = core.state.foreground_info().unwrap();
        assert_eq!(info.state, SessionState::Foreground);
        assert!(info.title.as_deref().unwrap().contains("ok.example"));
        assert_eq!(core.state.recents.len(), 1);
    }

    /// Why: the engine's security report must reach the UI state through the manager, and
    /// the previous page's verdict must not be shown while the next page loads.
    #[test]
    fn engine_security_reports_reach_the_ui_and_reset_on_the_next_navigation() {
        use krypa_engine_api::ConnectionSecurity as Sec;
        let mut core = Core::new().unwrap();
        let t0 = Instant::now();
        core.handle_intents(vec![nav("https://ok.example/")], t0);
        core.pump(t0 + Duration::from_millis(100));
        assert_eq!(core.state.foreground_info().unwrap().security, None);
        run(&mut core, t0);
        assert_eq!(
            core.state.foreground_info().unwrap().security,
            Some(Sec::Secure)
        );
        let id = core.state.foreground.unwrap();
        // Same session, new page (an Intent::Navigate would open a new session).
        let out = core
            .manager
            .navigate(id, Url::parse("http://plain.test/").unwrap())
            .unwrap();
        core.apply(out, t0);
        core.pump(t0 + Duration::from_millis(100));
        assert_eq!(
            core.state.foreground_info().unwrap().security,
            None,
            "the old page's Secure must not outlive the new navigation start"
        );
        run(&mut core, t0);
        assert_eq!(
            core.state.foreground_info().unwrap().security,
            Some(Sec::Insecure)
        );
        core.handle_intents(
            vec![Intent::Navigate(
                NavigationUrl::parse("https://badcert.test/").unwrap(),
            )],
            t0,
        );
        run(&mut core, t0);
        assert_eq!(
            core.state.foreground_info().unwrap().security,
            Some(Sec::CertificateError)
        );
    }

    #[test]
    fn a_crash_becomes_a_failed_session_and_reload_recovers_it() {
        let mut core = Core::new().unwrap();
        let t0 = Instant::now();
        core.handle_intents(vec![nav("https://ok.example/")], t0);
        run(&mut core, t0);
        core.crash_foreground(t0);
        let id = core.state.foreground.unwrap();
        assert_eq!(
            core.state.foreground_info().unwrap().state,
            SessionState::Failed
        );
        core.handle_intents(vec![Intent::Reload(id)], t0);
        run(&mut core, t0);
        assert_eq!(
            core.state.foreground_info().unwrap().state,
            SessionState::Foreground
        );
    }

    #[test]
    fn memory_pressure_terminates_background_views_but_never_the_foreground() {
        let mut core = Core::new().unwrap();
        let t0 = Instant::now();
        for host in ["https://a.example/", "https://b.example/"] {
            core.handle_intents(vec![nav(host)], t0);
            run(&mut core, t0);
        }
        core.memory_pressure(MemoryPressure::Critical, t0);
        run(&mut core, t0);
        let states: Vec<_> = core.state.sessions.iter().map(|s| s.state).collect();
        assert!(states.contains(&SessionState::Terminated));
        assert_eq!(
            core.state.foreground_info().unwrap().state,
            SessionState::Foreground
        );
        assert_eq!(core.engine.view_count(), core.manager.live_view_count());
    }

    #[test]
    fn a_permission_prompt_reaches_the_ui_and_the_answer_reaches_the_manager() {
        let mut core = Core::new().unwrap();
        let t0 = Instant::now();
        core.handle_intents(vec![nav("https://ok.example/")], t0);
        run(&mut core, t0);
        core.request_permission(PermissionKind::Camera, t0);
        let prompt = core.state.permission_prompts.first().cloned().unwrap();
        core.handle_intents(
            vec![Intent::RespondPermission {
                session: prompt.session,
                request: prompt.request,
                decision: PermissionDecision::Deny,
            }],
            t0,
        );
        assert!(core.status.is_empty(), "{}", core.status);
    }

    /// Why: "Add to Krypa" no longer installs on the spot. The engine is asked for the page's
    /// manifest, the user sees what will be installed, and only a confirm saves it.
    #[test]
    fn adding_a_page_creates_an_installed_app_that_reopens_in_scope() {
        let mut core = Core::new().unwrap();
        let t0 = Instant::now();
        core.handle_intents(vec![nav("https://shop.example/cart")], t0);
        run(&mut core, t0);
        let id = core.state.foreground.unwrap();
        core.handle_intents(vec![Intent::AddToKrypa(id)], t0);
        assert!(
            core.state.installed_apps.is_empty(),
            "nothing is installed before the user confirms"
        );
        run(&mut core, t0);
        let prompt = core.state.install_prompt.take().unwrap();
        core.handle_intents(vec![Intent::ConfirmInstall(Box::new(prompt.app))], t0);
        assert_eq!(core.state.installed_apps.len(), 1);
        let app_id = core.state.installed_apps[0].id().clone();
        core.handle_intents(vec![Intent::OpenApp(app_id)], t0);
        assert_eq!(core.state.sessions.len(), 2);
    }

    fn open_and_ask(core: &mut Core, page: &str, t0: Instant) -> krypa_ui::InstallPrompt {
        core.handle_intents(vec![nav(page)], t0);
        run(core, t0);
        let id = core.state.foreground.unwrap();
        core.handle_intents(vec![Intent::AddToKrypa(id)], t0);
        run(core, t0);
        core.state
            .install_prompt
            .clone()
            .expect("an install prompt after the engine answered")
    }

    /// Why: the real name, scope, display and colours come from the manifest; the icon list
    /// is validated (the plain-http icon in the fake manifest must be gone).
    #[test]
    fn a_page_with_a_manifest_offers_the_manifest_app() {
        let mut core = Core::new().unwrap();
        let p = open_and_ask(&mut core, "https://shop.example/manifest/", Instant::now());
        assert_eq!(p.source, InstallSource::Manifest);
        assert_eq!(p.app.name(), "Fake Shop");
        assert_eq!(p.app.scope().path(), "/manifest/");
        assert_eq!(p.app.icons().len(), 2, "the http icon was dropped");
        assert!(p.app.icons().iter().all(|i| i.src().scheme() == "https"));
    }

    /// Why: owner decision, installed apps are https-only. "Add to Krypa" on the harness's
    /// plain-http sample must show the refusal, never a dialog, and must leave nothing to
    /// confirm; the https sample next to it still installs end to end.
    #[test]
    fn add_to_krypa_on_a_plain_http_page_shows_the_refusal_not_a_dialog() {
        use krypa_core::InstallRefusal;
        let mut core = Core::new().unwrap();
        let t0 = Instant::now();
        core.handle_intents(vec![nav("http://insecure.test/")], t0);
        run(&mut core, t0);
        let id = core.state.foreground.unwrap();
        core.handle_intents(vec![Intent::AddToKrypa(id)], t0);
        run(&mut core, t0);
        assert_eq!(core.state.install_refusal, Some(InstallRefusal::NotHttps));
        assert!(core.state.install_prompt.is_none());
        assert!(core.state.installed_apps.is_empty());
    }

    /// Why: a certificate-error page never commits, so it has nothing to install; the
    /// refusal must not leave a dialog behind either.
    #[test]
    fn add_to_krypa_on_a_page_that_failed_its_certificate_check_installs_nothing() {
        let mut core = Core::new().unwrap();
        let t0 = Instant::now();
        core.handle_intents(vec![nav("https://badcert.test/")], t0);
        run(&mut core, t0);
        let id = core.state.foreground.unwrap();
        core.handle_intents(vec![Intent::AddToKrypa(id)], t0);
        run(&mut core, t0);
        assert!(core.state.install_prompt.is_none());
        assert!(core.state.installed_apps.is_empty());
    }

    /// Why: a hostile manifest name may only ever be displayed after sanitizing.
    #[test]
    fn a_hostile_manifest_name_is_sanitized_before_the_user_sees_it() {
        let mut core = Core::new().unwrap();
        let p = open_and_ask(
            &mut core,
            "https://shop.example/manifest/hostile",
            Instant::now(),
        );
        assert!(!p.app.name().contains('\u{202E}'));
        assert!(p.app.name().chars().count() <= krypa_apps::MAX_NAME_CHARS);
    }

    /// Why: no manifest, a broken one and one that is too large must all still allow the
    /// install (as a shortcut to the page) and must never install the broken data.
    #[test]
    fn missing_broken_or_oversize_manifests_fall_back_to_a_shortcut() {
        for (page, reason) in [
            ("https://ok.example/", ManualReason::NoManifest),
            ("https://ok.example/badmanifest", ManualReason::Invalid),
            ("https://ok.example/bigmanifest", ManualReason::TooLarge),
        ] {
            let mut core = Core::new().unwrap();
            let p = open_and_ask(&mut core, page, Instant::now());
            assert_eq!(p.source, InstallSource::Manual(reason), "{page}");
            assert_eq!(p.app.start_url().as_str(), page, "{page}");
            assert!(p.app.icons().is_empty(), "{page}");
        }
    }

    /// Why: an answer that arrives after the session moved on belongs to the old page.
    #[test]
    fn a_manifest_answer_that_arrives_after_navigation_is_not_offered() {
        let mut core = Core::new().unwrap();
        let t0 = Instant::now();
        core.handle_intents(vec![nav("https://shop.example/manifest/")], t0);
        run(&mut core, t0);
        let id = core.state.foreground.unwrap();
        let out = core.manager.request_install(id).unwrap();
        let Some(EngineCommand::FetchManifest { request, .. }) = out.commands.first().cloned()
        else {
            panic!("expected a manifest request");
        };
        let out = core
            .manager
            .navigate(id, Url::parse("https://other.example/").unwrap())
            .unwrap();
        core.apply(out, t0);
        run(&mut core, t0);
        core.deliver(
            EngineEvent::ManifestFetched {
                session: id,
                request,
                result: ManifestResult::Found {
                    manifest_url: Url::parse("https://shop.example/manifest.webmanifest").unwrap(),
                    bytes: br#"{"name":"Old page"}"#.to_vec(),
                },
            },
            t0,
        );
        assert!(core.state.install_prompt.is_none());
    }

    fn temp_db() -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("krypa.sqlite3");
        (dir, path)
    }

    fn core_with_db(path: &std::path::Path) -> Core {
        let mut core = Core::new().unwrap();
        core.attach_storage(Storage::open(path).unwrap());
        core
    }

    /// Why: the point of installing is that it is still there tomorrow. The reloaded app
    /// must be the very same validated app, icons and colours included.
    #[test]
    fn an_installed_app_survives_a_restart() {
        let (_dir, path) = temp_db();
        let t0 = Instant::now();
        let installed = {
            let mut core = core_with_db(&path);
            let p = open_and_ask(&mut core, "https://shop.example/manifest/", t0);
            core.handle_intents(vec![Intent::ConfirmInstall(Box::new(p.app.clone()))], t0);
            assert_eq!(core.state.installed_apps, vec![p.app.clone()]);
            p.app
        };
        let core = core_with_db(&path);
        assert!(
            core.startup_problems.is_empty(),
            "{:?}",
            core.startup_problems
        );
        assert_eq!(core.state.installed_apps, vec![installed]);
        assert!(core.status.is_empty());
    }

    /// Why: a corrupt row must be reported, not dropped silently, and must not hide the
    /// good apps next to it.
    #[test]
    fn a_corrupt_stored_app_is_reported_at_startup_and_the_others_still_load() {
        let (_dir, path) = temp_db();
        let t0 = Instant::now();
        {
            let mut core = core_with_db(&path);
            let p = open_and_ask(&mut core, "https://shop.example/manifest/", t0);
            core.handle_intents(vec![Intent::ConfirmInstall(Box::new(p.app))], t0);
        }
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute(
            "INSERT INTO installed_apps (id, name, start_url, scope, display, installed_at)
             VALUES ('https://evil.example/', '', 'https://other.example/', 'https://evil.example/',
                     'browser', 0)",
            [],
        )
        .unwrap();
        drop(conn);
        let core = core_with_db(&path);
        assert_eq!(core.state.installed_apps.len(), 1, "the good app loads");
        assert_eq!(core.startup_problems.len(), 1);
        assert!(
            core.status.contains("could not be loaded"),
            "{}",
            core.status
        );
    }

    /// Why: icon bytes are hostile and fetched off the UI thread; whatever the host returns,
    /// the outcome must be a decoded image or a recorded failure, never a hang or a panic.
    #[test]
    fn icons_are_fetched_off_thread_and_failures_leave_the_placeholder() {
        let t0 = Instant::now();
        for (icon, ok) in [
            ("https://img.example/icons/ok.png", true),
            ("https://img.example/icons/bad.png", false),
            ("https://img.example/icons/huge.png", false),
            ("https://fail.example/icons/ok.png", false),
        ] {
            let mut core = Core::new().unwrap();
            let url = Url::parse(icon).unwrap();
            core.state.icons.wanted(&[app_with_icon(icon)], 132);
            core.handle_intents(vec![Intent::FetchIcon(url.clone())], t0);
            let started = Instant::now();
            while core.icons_pending() {
                assert!(
                    started.elapsed() < Duration::from_secs(10),
                    "icon job never finished"
                );
                std::thread::sleep(Duration::from_millis(5));
                core.pump(t0);
            }
            core.pump(t0);
            core.state.icons.upload(&eframe::egui::Context::default());
            assert_eq!(core.state.icons.texture(&url).is_some(), ok, "{icon}");
            assert_eq!(core.state.icons.failure(&url).is_some(), !ok, "{icon}");
        }
    }

    fn app_with_icon(icon: &str) -> InstalledApp {
        let doc = Url::parse("https://img.example/").unwrap();
        let m = krypa_apps::Manifest::parse(&format!(
            r#"{{"name":"Img","icons":[{{"src":"{icon}","sizes":"192x192"}}]}}"#
        ))
        .unwrap();
        InstalledApp::from_manifest(&doc, &doc, &m).unwrap()
    }

    fn page_with_tel(core: &mut Core, t0: Instant) {
        core.handle_intents(vec![nav("https://ok.example/")], t0);
        run(core, t0);
    }

    /// Why: nothing may reach the OS unless the user said yes, and Cancel must really do
    /// nothing.
    #[test]
    fn an_external_link_reaches_the_os_only_after_the_user_allows_it() {
        let t0 = Instant::now();
        let mut core = Core::new().unwrap();
        page_with_tel(&mut core, t0);
        let session = core.state.foreground.unwrap();

        core.page_requests_external("tel:+4312345", true, t0);
        assert!(core.state.external_prompt.is_some());
        assert!(core.engine.os_handoffs.is_empty(), "asked, not opened");
        core.handle_intents(
            vec![Intent::RespondExternalOpen {
                session,
                allow: false,
            }],
            t0,
        );
        assert!(core.state.external_prompt.is_none());
        assert!(core.engine.os_handoffs.is_empty(), "Cancel opens nothing");

        core.page_requests_external("tel:+4312345", true, t0);
        core.handle_intents(
            vec![Intent::RespondExternalOpen {
                session,
                allow: true,
            }],
            t0,
        );
        assert_eq!(
            core.engine.os_handoffs,
            vec![Url::parse("tel:+4312345").unwrap()]
        );
    }

    /// Why: no gesture, a blocked scheme and a repeat must not produce prompts.
    #[test]
    fn external_requests_without_a_gesture_with_blocked_schemes_or_repeated_make_no_extra_prompt() {
        let t0 = Instant::now();
        let mut core = Core::new().unwrap();
        page_with_tel(&mut core, t0);
        core.page_requests_external("tel:+4312345", false, t0);
        assert!(core.state.external_prompt.is_none(), "no gesture");
        for blocked in [
            "javascript:alert(1)",
            "data:text/html,x",
            "file:///etc/passwd",
        ] {
            core.status.clear();
            core.page_requests_external(blocked, true, t0);
            assert!(core.state.external_prompt.is_none(), "{blocked}");
            assert!(core.status.contains("refused"), "{}", core.status);
        }
        core.page_requests_external("tel:+1111111", true, t0);
        core.page_requests_external("tel:+2222222", true, t0);
        let prompt = core.state.external_prompt.clone().unwrap();
        assert_eq!(prompt.url.as_str(), "tel:+1111111", "the first one stays");
    }
}
