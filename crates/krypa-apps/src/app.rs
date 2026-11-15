use url::Url;

use crate::color::Rgba;
use crate::error::{ManifestError, StoredAppError};
use crate::icon::{select_best, Icon, IconPurpose, IconPurposes, IconSize, MAX_SIZE_TOKENS};
use crate::manifest::{Manifest, MAX_ICONS};

/// Longest app name kept, in characters.
pub const MAX_NAME_CHARS: usize = 45;

/// How an installed app is presented. Only what Krypa supports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisplayMode {
    /// App chrome only. `fullscreen` in a manifest degrades to this.
    Standalone,
    /// App chrome plus a minimal navigation bar.
    MinimalUi,
    /// A normal browser tab. Also the fallback for missing or unknown values.
    Browser,
}

/// The persisted fields of an installed app, as read back from storage. Untrusted: it
/// is only a carrier for [`InstalledApp::from_stored`], which re-validates everything.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredApp {
    /// App identity URL, in canonical serialized form.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Start URL, in canonical serialized form.
    pub start_url: String,
    /// Scope URL, in canonical serialized form.
    pub scope: String,
    /// Display mode.
    pub display: DisplayMode,
    /// Theme colour.
    pub theme_color: Option<Rgba>,
    /// Background colour.
    pub background_color: Option<Rgba>,
    /// Icons in manifest order.
    pub icons: Vec<StoredIcon>,
}

/// The persisted fields of one icon. Untrusted, see [`StoredApp`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredIcon {
    /// Absolute icon URL, in canonical serialized form.
    pub src: String,
    /// Declared sizes.
    pub sizes: Vec<IconSize>,
    /// Declared purposes.
    pub purposes: IconPurposes,
}

/// An installed web app. Every value has been validated: `id`, `start_url` and `scope`
/// are `http(s)` URLs of the same origin as the page it was installed from, `start_url`
/// lies inside `scope`, and the name is free of control and bidi characters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledApp {
    id: Url,
    name: String,
    start_url: Url,
    scope: Url,
    display: DisplayMode,
    theme_color: Option<Rgba>,
    background_color: Option<Rgba>,
    icons: Vec<Icon>,
}

impl InstalledApp {
    /// Builds an app from a parsed manifest.
    ///
    /// `document_url` is the page that advertised the manifest and defines the app's
    /// origin; `manifest_url` is where the manifest was fetched from and is the base for
    /// relative `start_url`, `scope` and icon `src` values.
    ///
    /// # Errors
    /// [`ManifestError::UnsupportedDocumentUrl`] / [`ManifestError::UnsupportedManifestUrl`]
    /// when either URL is not `http(s)` with a host and no credentials. Bad manifest
    /// members never fail; they fall back to defaults.
    pub fn from_manifest(
        document_url: &Url,
        manifest_url: &Url,
        manifest: &Manifest,
    ) -> Result<Self, ManifestError> {
        if !is_plain_web_url(document_url) {
            return Err(ManifestError::UnsupportedDocumentUrl);
        }
        if !is_plain_web_url(manifest_url) {
            return Err(ManifestError::UnsupportedManifestUrl);
        }
        let start_url = resolve_start_url(document_url, manifest_url, manifest);
        let scope = resolve_scope(manifest_url, manifest, &start_url);
        let id = resolve_id(manifest, &start_url);
        let name = [manifest.name.as_deref(), manifest.short_name.as_deref()]
            .into_iter()
            .flatten()
            .map(sanitize_name)
            .find(|n| !n.is_empty())
            .unwrap_or_else(|| host_name(document_url));
        Ok(Self {
            id,
            name,
            start_url,
            scope,
            display: parse_display(manifest.display.as_deref()),
            theme_color: manifest.theme_color.as_deref().and_then(Rgba::parse),
            background_color: manifest.background_color.as_deref().and_then(Rgba::parse),
            icons: manifest
                .icons
                .iter()
                .filter_map(|raw| Icon::from_raw(manifest_url, raw))
                .collect(),
        })
    }

    /// Builds an app for a site without a manifest: it starts at the page itself, the
    /// scope is that page's directory, and it is displayed as a browser tab. `title` is
    /// sanitized; if nothing is left the host name is used.
    ///
    /// # Errors
    /// [`ManifestError::UnsupportedDocumentUrl`] when `document_url` is not `http(s)`
    /// with a host and no credentials.
    pub fn manual(document_url: &Url, title: Option<&str>) -> Result<Self, ManifestError> {
        if !is_plain_web_url(document_url) {
            return Err(ManifestError::UnsupportedDocumentUrl);
        }
        let start_url = without_fragment(document_url);
        let name = title
            .map(sanitize_name)
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| host_name(document_url));
        Ok(Self {
            id: start_url.clone(),
            name,
            scope: default_scope(&start_url),
            start_url,
            display: DisplayMode::Browser,
            theme_color: None,
            background_color: None,
            icons: Vec::new(),
        })
    }

    /// Rebuilds an app from its stored fields, enforcing the same invariants as
    /// [`InstalledApp::from_manifest`]: `id`, `start_url` and `scope` are plain `http(s)`
    /// URLs (host, no credentials) of one origin; `start_url` lies inside `scope`; `scope`
    /// has no query or fragment and `id` no fragment; the name is non-empty and already
    /// sanitized; icons are plain `https` URLs, at most [`MAX_ICONS`], with valid sizes
    /// and at least one purpose. URL fields must also be in canonical serialized form, so
    /// a row is never silently rewritten. Nothing is trusted because it came from disk.
    ///
    /// # Errors
    /// [`StoredAppError`] naming the first field that violates an invariant.
    pub fn from_stored(stored: StoredApp) -> Result<Self, StoredAppError> {
        let bad = |field| StoredAppError { field };
        let id = canonical_web_url(&stored.id).ok_or(bad("id"))?;
        let start_url = canonical_web_url(&stored.start_url).ok_or(bad("start_url"))?;
        let scope = canonical_web_url(&stored.scope).ok_or(bad("scope"))?;
        if !same_origin(&id, &start_url) || id.fragment().is_some() {
            return Err(bad("id"));
        }
        if !same_origin(&scope, &start_url)
            || scope.query().is_some()
            || scope.fragment().is_some()
            || !in_scope(&scope, &start_url)
        {
            return Err(bad("scope"));
        }
        if stored.name.is_empty() || sanitize_name(&stored.name) != stored.name {
            return Err(bad("name"));
        }
        if stored.icons.len() > MAX_ICONS {
            return Err(bad("icons"));
        }
        let icons = stored
            .icons
            .into_iter()
            .map(|i| {
                let p = i.purposes;
                let sizes_ok = i.sizes.len() <= MAX_SIZE_TOKENS
                    && i.sizes.iter().all(|s| match s {
                        IconSize::Any => true,
                        IconSize::Px { width, height } => *width > 0 && *height > 0,
                    });
                let src = canonical_web_url(&i.src).filter(|u| u.scheme() == "https");
                match src {
                    Some(src) if sizes_ok && (p.any || p.maskable || p.monochrome) => Ok(Icon {
                        src,
                        sizes: i.sizes,
                        purposes: p,
                    }),
                    _ => Err(bad("icons")),
                }
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            id,
            name: stored.name,
            start_url,
            scope,
            display: stored.display,
            theme_color: stored.theme_color,
            background_color: stored.background_color,
            icons,
        })
    }

    /// The fields to persist; what [`InstalledApp::from_stored`] reads back.
    #[must_use]
    pub fn to_stored(&self) -> StoredApp {
        StoredApp {
            id: self.id.as_str().to_owned(),
            name: self.name.clone(),
            start_url: self.start_url.as_str().to_owned(),
            scope: self.scope.as_str().to_owned(),
            display: self.display,
            theme_color: self.theme_color,
            background_color: self.background_color,
            icons: self
                .icons
                .iter()
                .map(|i| StoredIcon {
                    src: i.src.as_str().to_owned(),
                    sizes: i.sizes.clone(),
                    purposes: i.purposes,
                })
                .collect(),
        }
    }

    /// Whether `url` is inside this app: same origin as the scope and a path that starts
    /// with the scope's path. Non-web schemes and other origins are never in scope.
    #[must_use]
    pub fn is_in_scope(&self, url: &Url) -> bool {
        in_scope(&self.scope, url)
    }

    /// The best icon with `purpose` for rendering at `target_px`, if any.
    #[must_use]
    pub fn best_icon(&self, target_px: u32, purpose: IconPurpose) -> Option<&Icon> {
        select_best(&self.icons, target_px, purpose)
    }

    /// The stable identity of the app, same origin as `start_url`.
    #[must_use]
    pub fn id(&self) -> &Url {
        &self.id
    }

    /// The sanitized display name; never empty.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The URL the app opens at.
    #[must_use]
    pub fn start_url(&self) -> &Url {
        &self.start_url
    }

    /// The navigation scope; contains `start_url`.
    #[must_use]
    pub fn scope(&self) -> &Url {
        &self.scope
    }

    /// The display mode.
    #[must_use]
    pub fn display(&self) -> DisplayMode {
        self.display
    }

    /// The theme colour, if the manifest gave a valid one.
    #[must_use]
    pub fn theme_color(&self) -> Option<Rgba> {
        self.theme_color
    }

    /// The background colour, if the manifest gave a valid one.
    #[must_use]
    pub fn background_color(&self) -> Option<Rgba> {
        self.background_color
    }

    /// The validated icons, in manifest order.
    #[must_use]
    pub fn icons(&self) -> &[Icon] {
        &self.icons
    }
}

/// Makes site-supplied text safe to show as an app name: whitespace runs (including
/// newlines) become one space, control characters, bidi overrides/isolates/marks and
/// other invisible format characters are removed, the result is trimmed and cut to
/// [`MAX_NAME_CHARS`] characters. May return an empty string.
#[must_use]
pub fn sanitize_name(raw: &str) -> String {
    let mut out = String::new();
    let mut count = 0usize;
    let mut pending_space = false;
    for c in raw.chars() {
        if c.is_whitespace() {
            pending_space = count > 0;
            continue;
        }
        if is_disallowed_char(c) {
            continue;
        }
        let needed = if pending_space { 2 } else { 1 };
        if count + needed > MAX_NAME_CHARS {
            break;
        }
        if pending_space {
            out.push(' ');
            pending_space = false;
        }
        out.push(c);
        count += needed;
    }
    out
}

fn is_disallowed_char(c: char) -> bool {
    c.is_control()
        || matches!(
            c,
            '\u{00AD}'
                | '\u{061C}'
                | '\u{180E}'
                | '\u{200B}'..='\u{200F}'
                | '\u{202A}'..='\u{202E}'
                | '\u{2060}'..='\u{2064}'
                | '\u{2066}'..='\u{2069}'
                | '\u{FEFF}'
                | '\u{E0000}'..='\u{E007F}'
        )
}

/// `http(s)`, has a host, no userinfo.
pub(crate) fn is_plain_web_url(url: &Url) -> bool {
    matches!(url.scheme(), "http" | "https")
        && url.host_str().is_some()
        && url.username().is_empty()
        && url.password().is_none()
}

/// Parses `text` as a plain web URL that re-serializes to exactly `text`.
fn canonical_web_url(text: &str) -> Option<Url> {
    Url::parse(text)
        .ok()
        .filter(|u| is_plain_web_url(u) && u.as_str() == text)
}

fn same_origin(a: &Url, b: &Url) -> bool {
    let origin = a.origin();
    origin.is_tuple() && origin == b.origin()
}

fn in_scope(scope: &Url, url: &Url) -> bool {
    is_plain_web_url(url) && same_origin(scope, url) && url.path().starts_with(scope.path())
}

fn without_fragment(url: &Url) -> Url {
    let mut u = url.clone();
    u.set_fragment(None);
    u
}

fn host_name(document_url: &Url) -> String {
    // Host is already punycode for IDN hosts, so it cannot hide look-alike characters.
    document_url.host_str().unwrap_or_default().to_owned()
}

/// A trimmed, non-empty member value, or `None` (an empty string would resolve to the
/// manifest URL itself, which is never what the author meant).
fn present(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|s| !s.is_empty())
}

fn resolve_start_url(document_url: &Url, manifest_url: &Url, manifest: &Manifest) -> Url {
    present(manifest.start_url.as_deref())
        .and_then(|s| manifest_url.join(s).ok())
        .filter(|u| is_plain_web_url(u) && same_origin(u, document_url))
        .unwrap_or_else(|| without_fragment(document_url))
}

fn default_scope(start_url: &Url) -> Url {
    let mut scope = start_url.clone();
    scope.set_query(None);
    scope.set_fragment(None);
    let dir = match scope.path().rfind('/') {
        Some(i) => scope.path()[..=i].to_owned(),
        None => "/".to_owned(),
    };
    scope.set_path(&dir);
    scope
}

fn resolve_scope(manifest_url: &Url, manifest: &Manifest, start_url: &Url) -> Url {
    present(manifest.scope.as_deref())
        .and_then(|s| manifest_url.join(s).ok())
        .filter(|u| is_plain_web_url(u) && same_origin(u, start_url))
        .map(|mut u| {
            u.set_query(None);
            u.set_fragment(None);
            u
        })
        .filter(|scope| in_scope(scope, start_url))
        .unwrap_or_else(|| default_scope(start_url))
}

fn resolve_id(manifest: &Manifest, start_url: &Url) -> Url {
    // Per spec the id is resolved against the origin of start_url, not its path.
    let mut base = start_url.clone();
    base.set_path("/");
    base.set_query(None);
    base.set_fragment(None);
    present(manifest.id.as_deref())
        .and_then(|s| base.join(s).ok())
        .filter(|u| is_plain_web_url(u) && same_origin(u, start_url))
        .map(|u| without_fragment(&u))
        .unwrap_or_else(|| without_fragment(start_url))
}

fn parse_display(raw: Option<&str>) -> DisplayMode {
    match raw.map(|s| s.trim().to_ascii_lowercase()).as_deref() {
        Some("standalone" | "fullscreen") => DisplayMode::Standalone,
        Some("minimal-ui") => DisplayMode::MinimalUi,
        _ => DisplayMode::Browser,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::icon::IconSize;

    fn u(s: &str) -> Url {
        Url::parse(s).unwrap()
    }

    fn doc() -> Url {
        u("https://app.example/")
    }

    fn manifest_url() -> Url {
        u("https://app.example/manifest.json")
    }

    fn install(json: &str) -> InstalledApp {
        let m = Manifest::parse(json).unwrap();
        InstalledApp::from_manifest(&doc(), &manifest_url(), &m).unwrap()
    }

    // ---- origin binding -------------------------------------------------------------

    #[test]
    fn a_manifest_cannot_install_an_app_pointing_at_another_origin() {
        for evil in [
            "https://evil.test/",
            "http://app.example/",
            "https://app.example:8443/",
            "https://sub.app.example/",
            "https://app.example@evil.test/",
            "//evil.test/",
            "javascript:alert(1)",
            "data:text/html,<script>alert(1)</script>",
            "file:///etc/passwd",
            "blob:https://app.example/uuid",
        ] {
            let app = install(&format!(r#"{{"start_url":"{evil}"}}"#));
            assert_eq!(app.start_url(), &doc(), "start_url {evil}");
            assert!(app.is_in_scope(app.start_url()));
            assert!(!app.is_in_scope(&u("https://evil.test/")));
        }
    }

    #[test]
    fn start_url_with_embedded_credentials_falls_back_to_the_document() {
        let app = install(r#"{"start_url":"https://user:pw@app.example/x"}"#);
        assert_eq!(app.start_url(), &doc());
    }

    #[test]
    fn a_same_origin_start_url_is_resolved_relative_to_the_manifest_url() {
        let m = Manifest::parse(r#"{"start_url":"../app/?utm=1#top"}"#).unwrap();
        let app = InstalledApp::from_manifest(
            &u("https://app.example/landing"),
            &u("https://app.example/static/m/manifest.json"),
            &m,
        )
        .unwrap();
        assert_eq!(
            app.start_url().as_str(),
            "https://app.example/static/app/?utm=1#top"
        );
        assert_eq!(app.scope().as_str(), "https://app.example/static/app/");
    }

    #[test]
    fn a_manifest_hosted_on_a_cdn_still_binds_start_url_to_the_document_origin() {
        let m = Manifest::parse(r#"{"start_url":"/home"}"#).unwrap();
        let app = InstalledApp::from_manifest(&doc(), &u("https://cdn.test/m.json"), &m).unwrap();
        // "/home" resolves against the CDN, which is another origin, so it is refused.
        assert_eq!(app.start_url(), &doc());
    }

    #[test]
    fn empty_start_url_does_not_resolve_to_the_manifest_file() {
        let app = install(r#"{"start_url":"   "}"#);
        assert_eq!(app.start_url(), &doc());
    }

    // ---- scope ----------------------------------------------------------------------

    #[test]
    fn cross_origin_scope_is_ignored_and_the_default_scope_applies() {
        for evil in [
            "https://evil.test/",
            "http://app.example/",
            "javascript:0",
            "//evil.test/",
        ] {
            let app = install(&format!(r#"{{"start_url":"/a/b.html","scope":"{evil}"}}"#));
            assert_eq!(
                app.scope().as_str(),
                "https://app.example/a/",
                "scope {evil}"
            );
            assert!(!app.is_in_scope(&u("https://evil.test/a/")));
        }
    }

    #[test]
    fn a_scope_that_does_not_contain_start_url_is_ignored() {
        let app = install(r#"{"start_url":"/a/b.html","scope":"/other/"}"#);
        assert_eq!(app.scope().as_str(), "https://app.example/a/");
        assert!(app.is_in_scope(app.start_url()));
    }

    #[test]
    fn a_declared_scope_that_contains_start_url_is_used() {
        let app = install(r#"{"start_url":"/a/b/c.html","scope":"/a/?q=1#f"}"#);
        assert_eq!(app.scope().as_str(), "https://app.example/a/");
        assert!(app.is_in_scope(&u("https://app.example/a/x/y")));
        assert!(!app.is_in_scope(&u("https://app.example/b/")));
    }

    #[test]
    fn an_absent_scope_defaults_to_the_directory_of_start_url() {
        let app = install(r#"{"start_url":"/a/b/page.html?x=1#y"}"#);
        assert_eq!(app.scope().as_str(), "https://app.example/a/b/");
        let root = install("{}");
        assert_eq!(root.scope().as_str(), "https://app.example/");
    }

    #[test]
    fn dot_segments_cannot_walk_out_of_the_scope() {
        let app = install(r#"{"start_url":"/app/","scope":"/app/"}"#);
        assert!(!app.is_in_scope(&u("https://app.example/app/../admin")));
        assert!(!app.is_in_scope(&u("https://app.example/app/%2e%2e/admin")));
    }

    #[test]
    fn is_in_scope_rejects_look_alike_hosts_and_non_web_urls() {
        let app = install("{}");
        for out in [
            "https://app.example.evil.test/",
            "https://app.example@evil.test/",
            "http://app.example/",
            "https://app.example:444/",
            "javascript:alert(1)",
            "data:text/html,x",
            "file:///x",
            "blob:https://app.example/1",
            "about:blank",
        ] {
            assert!(!app.is_in_scope(&u(out)), "{out}");
        }
        assert!(app.is_in_scope(&u("https://app.example/anything?q=1#f")));
    }

    // ---- id -------------------------------------------------------------------------

    #[test]
    fn id_defaults_to_start_url_without_its_fragment() {
        let app = install(r#"{"start_url":"/a/?x=1#frag"}"#);
        assert_eq!(app.id().as_str(), "https://app.example/a/?x=1");
    }

    #[test]
    fn id_is_resolved_against_the_origin_of_start_url() {
        let app = install(r#"{"start_url":"/a/b/","id":"stable?x#f"}"#);
        assert_eq!(app.id().as_str(), "https://app.example/stable?x");
    }

    #[test]
    fn a_cross_origin_id_cannot_claim_another_sites_identity() {
        for evil in [
            "https://evil.test/x",
            "//evil.test/x",
            "javascript:0",
            "http://app.example/x",
        ] {
            let app = install(&format!(r#"{{"start_url":"/a/","id":"{evil}"}}"#));
            assert_eq!(app.id().as_str(), "https://app.example/a/", "id {evil}");
        }
    }

    // ---- display --------------------------------------------------------------------

    #[test]
    fn display_degrades_to_what_krypa_supports() {
        let cases = [
            ("standalone", DisplayMode::Standalone),
            ("fullscreen", DisplayMode::Standalone),
            (" Minimal-UI ", DisplayMode::MinimalUi),
            ("browser", DisplayMode::Browser),
            ("window-controls-overlay", DisplayMode::Browser),
            ("garbage", DisplayMode::Browser),
            ("", DisplayMode::Browser),
        ];
        for (raw, expected) in cases {
            let app = install(&format!(r#"{{"display":"{raw}"}}"#));
            assert_eq!(app.display(), expected, "display {raw:?}");
        }
        assert_eq!(install("{}").display(), DisplayMode::Browser);
    }

    // ---- names ----------------------------------------------------------------------

    #[test]
    fn hostile_names_cannot_spoof_via_bidi_overrides() {
        let spoof = "\u{202E}gpj.exe\u{202C}";
        let name = sanitize_name(spoof);
        assert_eq!(name, "gpj.exe");
        for c in name.chars() {
            assert!(!('\u{202A}'..='\u{202E}').contains(&c));
        }
        let hostile = "Bank\u{202e}\u{2066} evil\u{2069}\u{200f}\u{200b}";
        let app = install(&format!("{{\"name\":\"{hostile}\"}}"));
        assert_eq!(app.name(), "Bank evil");
    }

    #[test]
    fn names_with_control_characters_are_cleaned() {
        assert_eq!(sanitize_name("a\u{0}b\u{7}c\u{1b}[31md\u{7f}"), "abc[31md");
        assert_eq!(sanitize_name("  My \n\t  App \r\n"), "My App");
        assert_eq!(sanitize_name("a\u{2028}b\u{00a0}c"), "a b c");
    }

    #[test]
    fn a_name_of_only_whitespace_or_invisible_characters_falls_back() {
        for hostile in [
            "   ",
            "\n\t\r",
            "\u{202E}\u{202C}",
            "\u{0}\u{1}\u{2}",
            "\u{200B}\u{FEFF}",
            "",
        ] {
            assert_eq!(sanitize_name(hostile), "", "{hostile:?}");
            let app = install(&format!(
                r#"{{"name":{}}}"#,
                serde_json::to_string(hostile).unwrap()
            ));
            assert_eq!(app.name(), "app.example", "{hostile:?}");
        }
    }

    #[test]
    fn name_falls_back_to_short_name_then_to_the_host() {
        assert_eq!(
            install(r#"{"name":" ","short_name":"Short"}"#).name(),
            "Short"
        );
        assert_eq!(
            install(r#"{"name":"Long","short_name":"Short"}"#).name(),
            "Long"
        );
        assert_eq!(
            install(r#"{"name":5,"short_name":false}"#).name(),
            "app.example"
        );
    }

    #[test]
    fn an_idn_host_fallback_name_is_shown_as_punycode() {
        let m = Manifest::parse("{}").unwrap();
        let d = u("https://b\u{fc}cher.example/");
        let app =
            InstalledApp::from_manifest(&d, &u("https://b\u{fc}cher.example/m.json"), &m).unwrap();
        assert_eq!(app.name(), "xn--bcher-kva.example");
    }

    #[test]
    fn overlong_names_are_cut_on_a_character_boundary() {
        let long = "\u{e9}".repeat(10_000);
        let name = sanitize_name(&long);
        assert_eq!(name.chars().count(), MAX_NAME_CHARS);
        let spaced = format!("{} {}", "a".repeat(MAX_NAME_CHARS - 1), "b");
        assert_eq!(sanitize_name(&spaced), "a".repeat(MAX_NAME_CHARS - 1));
    }

    // ---- whole-manifest hostility ---------------------------------------------------

    #[test]
    fn wrong_json_types_for_every_field_still_install_with_safe_defaults() {
        let app = install(
            r#"{"name":1,"short_name":[],"id":{},"start_url":true,"scope":null,
                "display":5,"theme_color":1.5,"background_color":[1],"icons":"x"}"#,
        );
        assert_eq!(app.name(), "app.example");
        assert_eq!(app.start_url(), &doc());
        assert_eq!(app.scope(), &doc());
        assert_eq!(app.id(), &doc());
        assert_eq!(app.display(), DisplayMode::Browser);
        assert_eq!(app.theme_color(), None);
        assert_eq!(app.background_color(), None);
        assert!(app.icons().is_empty());
    }

    #[test]
    fn unsupported_document_or_manifest_urls_are_refused() {
        let m = Manifest::default();
        for bad in [
            "javascript:alert(1)",
            "data:text/html,x",
            "file:///x",
            "blob:https://a.example/1",
            "ftp://a.example/",
            "https://user:pw@a.example/",
        ] {
            assert_eq!(
                InstalledApp::from_manifest(&u(bad), &manifest_url(), &m),
                Err(ManifestError::UnsupportedDocumentUrl),
                "{bad}"
            );
            assert_eq!(
                InstalledApp::manual(&u(bad), Some("x")),
                Err(ManifestError::UnsupportedDocumentUrl)
            );
            assert_eq!(
                InstalledApp::from_manifest(&doc(), &u(bad), &m),
                Err(ManifestError::UnsupportedManifestUrl),
                "{bad}"
            );
        }
    }

    #[test]
    fn invalid_colours_are_dropped_and_valid_ones_kept() {
        let app =
            install(r##"{"theme_color":"#336699","background_color":"expression(alert(1))"}"##);
        assert_eq!(
            app.theme_color(),
            Some(Rgba {
                r: 0x33,
                g: 0x66,
                b: 0x99,
                a: 255
            })
        );
        assert_eq!(app.background_color(), None);
    }

    // ---- manual ---------------------------------------------------------------------

    #[test]
    fn a_manual_install_of_a_site_without_a_manifest_stays_on_that_page() {
        let app = InstalledApp::manual(
            &u("https://site.example/docs/page.html?q=1#x"),
            Some("  Docs\u{202E} "),
        )
        .unwrap();
        assert_eq!(app.name(), "Docs");
        assert_eq!(
            app.start_url().as_str(),
            "https://site.example/docs/page.html?q=1"
        );
        assert_eq!(app.id(), app.start_url());
        assert_eq!(app.scope().as_str(), "https://site.example/docs/");
        assert_eq!(app.display(), DisplayMode::Browser);
        assert!(app.icons().is_empty());
        assert!(!app.is_in_scope(&u("https://site.example/")));
    }

    #[test]
    fn a_manual_install_with_a_hostile_or_missing_title_uses_the_host() {
        let d = u("https://site.example/");
        assert_eq!(
            InstalledApp::manual(&d, None).unwrap().name(),
            "site.example"
        );
        assert_eq!(
            InstalledApp::manual(&d, Some("\u{202E}\u{200B}"))
                .unwrap()
                .name(),
            "site.example"
        );
    }

    // ---- icons ----------------------------------------------------------------------

    #[test]
    fn javascript_and_data_icon_urls_are_dropped() {
        let app = install(
            r#"{"icons":[
                {"src":"javascript:alert(1)","sizes":"48x48"},
                {"src":"data:image/png;base64,AAAA","sizes":"48x48"},
                {"src":"file:///etc/passwd"},
                {"src":"blob:https://app.example/1"},
                {"src":"ftp://app.example/i.png"}]}"#,
        );
        assert!(app.icons().is_empty());
    }

    #[test]
    fn http_icons_on_an_https_site_are_dropped() {
        let app = install(
            r#"{"icons":[{"src":"http://app.example/i.png"},{"src":"//app.example/ok.png"}]}"#,
        );
        assert_eq!(app.icons().len(), 1);
        assert_eq!(app.icons()[0].src().as_str(), "https://app.example/ok.png");
    }

    #[test]
    fn icons_with_credentials_or_unknown_purposes_are_dropped() {
        let app = install(
            r#"{"icons":[
                {"src":"https://u:p@app.example/i.png"},
                {"src":"/i.png","purpose":"bogus"},
                {"src":"/ok.png","purpose":"bogus maskable"}]}"#,
        );
        assert_eq!(app.icons().len(), 1);
        assert!(app.icons()[0].purposes().maskable);
    }

    #[test]
    fn relative_icon_urls_resolve_against_the_manifest_url() {
        let m = Manifest::parse(r#"{"icons":[{"src":"i/a.png","sizes":"32x32"}]}"#).unwrap();
        let app =
            InstalledApp::from_manifest(&doc(), &u("https://app.example/static/manifest.json"), &m)
                .unwrap();
        assert_eq!(
            app.icons()[0].src().as_str(),
            "https://app.example/static/i/a.png"
        );
        assert_eq!(
            app.icons()[0].sizes(),
            [IconSize::Px {
                width: 32,
                height: 32
            }]
        );
    }

    #[test]
    fn icon_selection_prefers_the_smallest_icon_that_is_large_enough() {
        let app = install(
            r#"{"icons":[
                {"src":"/16.png","sizes":"16x16"},
                {"src":"/512.png","sizes":"512x512"},
                {"src":"/192.png","sizes":"192x192"},
                {"src":"/96.png","sizes":"96x96"}]}"#,
        );
        let pick = |px| {
            app.best_icon(px, IconPurpose::Any)
                .map(|i| i.src().path().to_owned())
        };
        assert_eq!(pick(100).as_deref(), Some("/192.png"));
        assert_eq!(pick(96).as_deref(), Some("/96.png"));
        assert_eq!(pick(1024).as_deref(), Some("/512.png"));
        assert_eq!(pick(1).as_deref(), Some("/16.png"));
    }

    #[test]
    fn icon_selection_honours_purpose_and_scalable_icons() {
        let app = install(
            r#"{"icons":[
                {"src":"/mask.png","sizes":"192x192","purpose":"maskable"},
                {"src":"/any.png","sizes":"48x48"},
                {"src":"/vec.svg","sizes":"any","purpose":"any monochrome"},
                {"src":"/nosize.png"}]}"#,
        );
        let pick = |px, p| app.best_icon(px, p).map(|i| i.src().path().to_owned());
        assert_eq!(
            pick(192, IconPurpose::Maskable).as_deref(),
            Some("/mask.png")
        );
        assert_eq!(pick(64, IconPurpose::Any).as_deref(), Some("/vec.svg"));
        assert_eq!(pick(48, IconPurpose::Any).as_deref(), Some("/any.png"));
        assert_eq!(
            pick(48, IconPurpose::Monochrome).as_deref(),
            Some("/vec.svg")
        );
        let none = install(r#"{"icons":[{"src":"/a.png","sizes":"48x48"}]}"#);
        assert!(none.best_icon(48, IconPurpose::Maskable).is_none());
        assert!(install("{}").best_icon(48, IconPurpose::Any).is_none());
    }

    #[test]
    fn an_icon_without_valid_sizes_is_chosen_only_as_a_last_resort() {
        let app =
            install(r#"{"icons":[{"src":"/nosize.png"},{"src":"/small.png","sizes":"16x16"}]}"#);
        let best = app.best_icon(64, IconPurpose::Any).unwrap();
        assert_eq!(best.src().path(), "/small.png");
    }

    // ---- reconstruction from stored fields --------------------------------------------

    const RICH: &str = r##"{"name":"Notes","id":"/notes","start_url":"/notes/home?x=1#top",
        "scope":"/notes/","display":"minimal-ui","theme_color":"#112233",
        "background_color":"rgba(1,2,3,0.5)","icons":[
        {"src":"/i/192.png","sizes":"192x192 any","purpose":"any maskable"},
        {"src":"/i/m.png","sizes":"48x48","purpose":"monochrome"},{"src":"/i/n.png"}]}"##;

    fn stored() -> StoredApp {
        let m = Manifest::parse(RICH).unwrap();
        let doc = u("https://notes.example/notes/");
        InstalledApp::from_manifest(&doc, &u("https://notes.example/m.json"), &m)
            .unwrap()
            .to_stored()
    }

    #[test]
    fn every_installable_shape_survives_to_stored_and_back_unchanged() {
        let doc = u("https://notes.example:8443/notes/");
        let mu = u("https://notes.example:8443/m.json");
        let manifests = [RICH, "{}", r#"{"scope":"/a/b/","start_url":"/a/b/c"}"#];
        for json in manifests {
            let m = Manifest::parse(json).unwrap();
            let app = InstalledApp::from_manifest(&doc, &mu, &m).unwrap();
            assert_eq!(InstalledApp::from_stored(app.to_stored()).unwrap(), app);
        }
        let manual = InstalledApp::manual(&u("https://x.example/a/b?q#f"), Some("X")).unwrap();
        assert_eq!(
            InstalledApp::from_stored(manual.to_stored()).unwrap(),
            manual
        );
        let http = InstalledApp::manual(&u("http://plain.example/"), None).unwrap();
        assert_eq!(InstalledApp::from_stored(http.to_stored()).unwrap(), http);
    }

    fn field_of(s: StoredApp) -> &'static str {
        InstalledApp::from_stored(s).unwrap_err().field
    }

    #[test]
    fn a_stored_scope_outside_the_origin_or_not_containing_start_is_refused() {
        // The scope decides what stays in the app and what is external.
        for bad in [
            "https://evil.example/notes/",
            "http://notes.example/notes/",
            "https://notes.example:444/notes/",
            "https://notes.example/other/",
            "https://notes.example/notes/?q=1",
            "https://notes.example/notes/#f",
            "javascript:alert(1)",
            "https://u:p@notes.example/notes/",
            "not a url",
            "",
        ] {
            let mut s = stored();
            s.scope = bad.to_owned();
            assert_eq!(field_of(s), "scope", "{bad}");
        }
    }

    #[test]
    fn a_stored_id_or_start_url_that_is_off_origin_hostile_or_non_canonical_is_refused() {
        for (id, start) in [
            (
                "https://evil.example/notes",
                "https://notes.example/notes/home",
            ),
            (
                "https://notes.example/notes",
                "https://evil.example/notes/home",
            ),
            ("file:///etc/passwd", "https://notes.example/notes/home"),
            ("https://notes.example/notes", "data:text/html,x"),
            (
                "https://notes.example/notes#frag",
                "https://notes.example/notes/home",
            ),
            (
                "https://notes.example/notes",
                "https://NOTES.example/notes/home",
            ),
        ] {
            let mut s = stored();
            s.id = id.to_owned();
            s.start_url = start.to_owned();
            assert!(InstalledApp::from_stored(s).is_err(), "{id} {start}");
        }
    }

    #[test]
    fn a_stored_name_that_sanitizing_would_change_is_refused() {
        let long = "x".repeat(MAX_NAME_CHARS + 1);
        for bad in ["", "  padded ", "bad\u{202e}name", "two\nlines", &long] {
            let mut s = stored();
            s.name = bad.to_owned();
            assert_eq!(field_of(s), "name");
        }
    }

    #[test]
    fn stored_icons_must_still_be_plain_https_with_valid_sizes_and_a_purpose() {
        let mut cases = Vec::new();
        for src in [
            "http://notes.example/i.png",
            "javascript:alert(1)",
            "data:image/png;base64,AA",
            "https://u@notes.example/i.png",
        ] {
            let mut s = stored();
            s.icons[0].src = src.to_owned();
            cases.push(s);
        }
        let mut s = stored();
        s.icons[0].purposes = IconPurposes {
            any: false,
            maskable: false,
            monochrome: false,
        };
        cases.push(s);
        let mut s = stored();
        s.icons[0].sizes = vec![IconSize::Px {
            width: 0,
            height: 8,
        }];
        cases.push(s);
        let mut s = stored();
        s.icons = vec![s.icons[0].clone(); MAX_ICONS + 1];
        cases.push(s);
        for s in cases {
            assert_eq!(field_of(s), "icons");
        }
    }
}
