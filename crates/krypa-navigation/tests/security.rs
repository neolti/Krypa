//! Security-intent tests for `krypa-navigation`. Test names state the property
//! being protected, not the function being called.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use krypa_navigation::{
    classify_input, Input, NavigationError, NavigationUrl, Origin, Scheme, SearchConfig,
    SecurityState, MAX_INPUT_BYTES,
};

fn url_of(input: &str) -> NavigationUrl {
    match classify_input(input) {
        Ok(Input::Url(u)) => u,
        other => panic!("{input:?} should classify as a URL, got {other:?}"),
    }
}

fn search_of(input: &str) -> String {
    match classify_input(input) {
        Ok(Input::Search(s)) => s,
        other => panic!("{input:?} should classify as search, got {other:?}"),
    }
}

fn shown(input: &str) -> String {
    NavigationUrl::parse(input)
        .unwrap()
        .origin()
        .display_origin()
        .to_string()
}

fn assert_blocked(input: &str, scheme: &str) {
    assert_eq!(
        classify_input(input),
        Err(NavigationError::BlockedScheme(scheme.to_owned())),
        "classify_input({input:?})"
    );
    assert_eq!(
        NavigationUrl::parse(input),
        Err(NavigationError::BlockedScheme(scheme.to_owned())),
        "NavigationUrl::parse({input:?})"
    );
}

// ---- Dangerous schemes -------------------------------------------------------------

#[test]
fn a_javascript_url_can_never_become_a_navigation() {
    assert_blocked("javascript:alert(1)", "javascript");
    assert_blocked("javascript://%0aalert(1)", "javascript");
    assert_blocked("javascript: alert(1)", "javascript");
}

#[test]
fn mixed_case_javascript_is_still_blocked() {
    assert_blocked("JaVaScRiPt:alert(1)", "javascript");
    assert_blocked("JAVASCRIPT:alert(1)", "javascript");
}

#[test]
fn whitespace_padded_javascript_is_still_blocked() {
    assert_blocked("  javascript:alert(1)  ", "javascript");
    assert_blocked("\t\n javascript:alert(1)", "javascript");
    assert_blocked("\u{1}\u{2}javascript:alert(1)", "javascript");
    assert_blocked("\u{a0}javascript:alert(1)\u{2003}", "javascript");
}

#[test]
fn tabs_and_newlines_inside_the_scheme_do_not_hide_javascript() {
    // The URL standard deletes tab/newline anywhere in the input, so these load as javascript:.
    assert_blocked("java\tscript:alert(1)", "javascript");
    assert_blocked("java\nscript:alert(1)", "javascript");
    assert_blocked("j\ra\tv\na\nscript:alert(1)", "javascript");
}

#[test]
fn data_file_blob_and_friends_can_never_become_a_navigation() {
    assert_blocked("data:text/html,<script>alert(1)</script>", "data");
    assert_blocked("DATA:text/html;base64,PHNjcmlwdD4=", "data");
    assert_blocked("file:///etc/passwd", "file");
    assert_blocked("file://localhost/etc/passwd", "file");
    assert_blocked("blob:https://example.com/uuid", "blob");
    assert_blocked("vbscript:msgbox(1)", "vbscript");
    assert_blocked("about:blank", "about");
    assert_blocked("view-source:https://example.com", "view-source");
    assert_blocked("ftp://example.com/file", "ftp");
    assert_blocked("wss://example.com/socket", "wss");
}

#[test]
fn an_unknown_opaque_scheme_is_never_a_navigation() {
    // No authority and not a known OS handler: not loadable in the browser.
    assert!(matches!(
        NavigationUrl::parse("foo:bar"),
        Err(NavigationError::BlockedScheme(_))
    ));
}

#[test]
fn os_handled_schemes_are_reported_distinctly_so_the_user_is_asked() {
    for (text, scheme) in [
        ("mailto:someone@example.com", "mailto"),
        ("MAILTO:someone@example.com", "mailto"),
        ("tel:+4366012345", "tel"),
        ("sms:+4366012345", "sms"),
        ("spotify://track/abc", "spotify"),
    ] {
        match classify_input(text) {
            Ok(Input::External(link)) => assert_eq!(link.scheme(), scheme, "{text}"),
            other => panic!("{text:?} should be External, got {other:?}"),
        }
        assert!(
            matches!(
                NavigationUrl::parse(text),
                Err(NavigationError::ExternalScheme(_))
            ),
            "{text}"
        );
    }
}

#[test]
fn an_external_link_is_never_auto_loaded_even_with_hidden_characters() {
    assert!(matches!(
        classify_input("  ma\til\nto:a@b.test "),
        Ok(Input::External(_))
    ));
}

// ---- Userinfo spoofing and host confusion ---------------------------------------------

#[test]
fn userinfo_spoof_with_a_trusted_name_is_rejected() {
    for text in [
        "https://bank.com@evil.test",
        "https://bank.com:secret@evil.test/login",
        "http://bank.com@evil.test",
        "https://bank.com%2F@evil.test",
        "https://:pw@evil.test",
    ] {
        assert_eq!(
            NavigationUrl::parse(text),
            Err(NavigationError::EmbeddedCredentials),
            "{text}"
        );
        assert_eq!(
            classify_input(text),
            Err(NavigationError::EmbeddedCredentials),
            "{text}"
        );
    }
}

#[test]
fn scheme_less_userinfo_spoof_is_rejected_not_searched() {
    assert_eq!(
        classify_input("bank.com@evil.test"),
        Err(NavigationError::EmbeddedCredentials)
    );
}

#[test]
fn userinfo_spoof_shows_the_host_that_would_actually_load() {
    // Backslash is a path separator for http(s), so the engine loads bank.com, and so do we.
    assert_eq!(shown("https://bank.com\\@evil.test"), "bank.com");
    // Not a credential: evil.test is the host, whatever is in the path.
    assert_eq!(shown("https://evil.test/@bank.com"), "evil.test");
    assert_eq!(shown("https://evil.test/https://bank.com"), "evil.test");
    assert_eq!(shown("https://evil.test?bank.com"), "evil.test");
    assert_eq!(shown("https://evil.test#@bank.com"), "evil.test");
}

#[test]
fn a_trusted_name_used_as_a_subdomain_is_shown_in_full() {
    assert_eq!(shown("https://bank.com.evil.test/"), "bank.com.evil.test");
    assert_eq!(
        shown("https://login-bank.com.evil.test/"),
        "login-bank.com.evil.test"
    );
}

#[test]
fn the_display_origin_strips_nothing_that_identifies_the_site() {
    assert_eq!(shown("https://WWW.Example.COM/a?b#c"), "www.example.com");
    assert_eq!(
        shown("https://a.b.c.example.com:8443/"),
        "a.b.c.example.com:8443"
    );
}

#[test]
fn obfuscated_ip_hosts_are_displayed_as_the_real_address() {
    assert_eq!(shown("http://0x7f.1/"), "127.0.0.1");
    assert_eq!(shown("http://2130706433/"), "127.0.0.1");
    assert_eq!(shown("http://0177.0.0.1/"), "127.0.0.1");
}

// ---- IDN homographs --------------------------------------------------------------------

#[test]
fn a_cyrillic_homograph_of_paypal_is_displayed_as_punycode() {
    let url = NavigationUrl::parse("https://p\u{430}ypal.com/").unwrap();
    let display = url.origin().display_origin();
    assert!(display.shown_as_punycode);
    assert!(display.host.starts_with("xn--"), "{}", display.host);
    assert_ne!(display.host, "paypal.com");
}

#[test]
fn an_all_cyrillic_whole_script_homograph_is_displayed_as_punycode() {
    // "apple" written entirely in Cyrillic letters: single-script, still spoofable.
    let url = NavigationUrl::parse("https://\u{430}\u{440}\u{440}\u{4CF}\u{435}.com/").unwrap();
    assert!(url.origin().display_origin().shown_as_punycode);
    let url = NavigationUrl::parse("https://\u{430}\u{440}\u{440}\u{43B}\u{435}.com/").unwrap();
    assert!(url.origin().display_origin().host.starts_with("xn--"));
}

#[test]
fn greek_and_mixed_script_lookalikes_are_displayed_as_punycode() {
    let greek = NavigationUrl::parse("https://g\u{3BF}\u{3BF}gle.com/").unwrap();
    assert!(greek.origin().display_origin().shown_as_punycode);
    let mixed = NavigationUrl::parse("https://sub.\u{430}pple.com/").unwrap();
    assert!(mixed.origin().display_origin().host.contains("xn--"));
}

#[test]
fn dotless_i_lookalike_is_displayed_as_punycode() {
    let url = NavigationUrl::parse("https://pa\u{131}pal.com/").unwrap();
    assert!(url.origin().display_origin().shown_as_punycode);
}

#[test]
fn one_confusable_label_makes_the_whole_host_display_as_punycode() {
    let url = NavigationUrl::parse("https://www.p\u{430}ypal.example.com/").unwrap();
    let display = url.origin().display_origin();
    assert!(display.shown_as_punycode);
    assert!(display.host.starts_with("www.xn--"), "{}", display.host);
}

#[test]
fn a_legitimate_latin_idn_is_shown_readably_and_the_navigation_uses_punycode() {
    let url = url_of("b\u{FC}cher.de");
    assert_eq!(url.as_str(), "https://xn--bcher-kva.de/");
    let display = url.origin().display_origin();
    assert_eq!(display.host, "b\u{FC}cher.de");
    assert!(!display.shown_as_punycode);
}

#[test]
fn already_punycode_input_for_a_latin_idn_displays_the_same_as_unicode_input() {
    assert_eq!(shown("https://xn--bcher-kva.de/"), "b\u{FC}cher.de");
}

#[test]
fn malformed_punycode_never_panics_and_is_not_shown_as_unicode() {
    for text in [
        "https://xn--/",
        "https://xn--a.com/",
        "https://xn--zzzzzzzzzzzz.com/",
    ] {
        if let Ok(url) = NavigationUrl::parse(text) {
            let display = url.origin().display_origin();
            assert!(
                display.host.contains("xn--"),
                "{text} shown as {}",
                display.host
            );
        }
    }
}

// ---- Trailing dots, empty labels ---------------------------------------------------------

#[test]
fn a_trailing_dot_cannot_make_a_second_identity_for_the_same_site() {
    let plain = NavigationUrl::parse("https://example.com/").unwrap();
    let dotted = NavigationUrl::parse("https://example.com./").unwrap();
    assert_eq!(plain, dotted);
    assert_eq!(plain.origin(), dotted.origin());
    assert_eq!(url_of("example.com.").as_str(), "https://example.com/");
}

#[test]
fn empty_labels_and_dot_only_hosts_are_rejected() {
    for text in [
        "https://example.com../",
        "https://.example.com/",
        "https://a..b/",
        "https://./",
        "https://../",
        "https:///",
        "https://",
    ] {
        assert!(
            NavigationUrl::parse(text).is_err(),
            "{text} must be rejected"
        );
    }
}

#[test]
fn a_url_without_a_host_is_rejected() {
    assert!(NavigationUrl::parse("http://").is_err());
    assert!(NavigationUrl::parse("https://@/").is_err());
}

// ---- IPv6 and IPv4 literals ---------------------------------------------------------------

#[test]
fn ipv6_literal_is_a_url_and_displayed_in_brackets() {
    assert_eq!(shown("http://[::1]/"), "[::1]");
    assert_eq!(shown("https://[2001:db8::1]:8443/x"), "[2001:db8::1]:8443");
    assert_eq!(url_of("[::1]").as_str(), "https://[::1]/");
    assert_eq!(url_of("[::1]:8080").as_str(), "https://[::1]:8080/");
}

#[test]
fn ipv6_literals_with_credentials_or_garbage_are_rejected() {
    assert_eq!(
        NavigationUrl::parse("https://bank.com@[::1]/"),
        Err(NavigationError::EmbeddedCredentials)
    );
    for text in [
        "http://[::1",
        "http://[::1]x/",
        "http://[:::1]/",
        "http://[zz]/",
    ] {
        assert!(NavigationUrl::parse(text).is_err(), "{text}");
    }
}

#[test]
fn dotted_quad_is_a_url_but_version_numbers_and_decimals_are_search() {
    assert_eq!(url_of("192.168.1.1").as_str(), "https://192.168.1.1/");
    assert_eq!(
        url_of("192.168.1.1:8080/admin").as_str(),
        "https://192.168.1.1:8080/admin"
    );
    assert_eq!(search_of("3.14"), "3.14");
    assert_eq!(search_of("1.2.3"), "1.2.3");
    assert_eq!(search_of("999.1.1.1"), "999.1.1.1");
}

// ---- Ports ----------------------------------------------------------------------------------

#[test]
fn default_ports_are_not_part_of_what_the_indicator_shows() {
    assert_eq!(shown("https://example.com:443/"), "example.com");
    assert_eq!(shown("http://example.com:80/"), "example.com");
}

#[test]
fn a_non_default_port_is_always_shown() {
    assert_eq!(shown("https://example.com:8443/"), "example.com:8443");
    // 443 is not the default for http, so it is a different, visible origin.
    assert_eq!(shown("http://example.com:443/"), "example.com:443");
}

#[test]
fn explicit_default_port_is_the_same_origin_as_no_port() {
    let a = NavigationUrl::parse("https://example.com/")
        .unwrap()
        .origin();
    let b = NavigationUrl::parse("https://example.com:443/")
        .unwrap()
        .origin();
    assert_eq!(a, b);
    assert_eq!(a.port(), 443);
}

#[test]
fn scheme_host_and_port_each_distinguish_an_origin() {
    let base = NavigationUrl::parse("https://example.com/")
        .unwrap()
        .origin();
    for other in [
        "http://example.com/",
        "https://example.org/",
        "https://example.com:8443/",
        "https://sub.example.com/",
    ] {
        let o: Origin = NavigationUrl::parse(other).unwrap().origin();
        assert_ne!(base, o, "{other}");
    }
}

#[test]
fn invalid_ports_are_rejected() {
    for text in [
        "https://example.com:0/",
        "https://example.com:65536/",
        "https://example.com:-1/",
        "https://example.com:abc/",
    ] {
        assert!(
            NavigationUrl::parse(text).is_err(),
            "{text} must be rejected"
        );
    }
}

#[test]
fn an_empty_port_means_the_default_port() {
    // Legal per the URL standard; it must not produce a distinct origin.
    let empty = NavigationUrl::parse("https://example.com:/x").unwrap();
    assert_eq!(empty.origin().port(), 443);
}

#[test]
fn localhost_with_a_port_is_a_url_not_a_search() {
    assert_eq!(url_of("localhost:3000").as_str(), "https://localhost:3000/");
    assert_eq!(url_of("localhost").as_str(), "https://localhost/");
    assert_eq!(url_of("myhost:8080/x").as_str(), "https://myhost:8080/x");
    assert_eq!(
        url_of("http://localhost:3000").as_str(),
        "http://localhost:3000/"
    );
}

// ---- Very long and degenerate input -----------------------------------------------------------

#[test]
fn input_over_the_cap_is_rejected_by_every_entry_point() {
    let long_url = format!("https://example.com/{}", "a".repeat(MAX_INPUT_BYTES));
    let long_text = "word ".repeat(MAX_INPUT_BYTES);
    for text in [&long_url, &long_text] {
        assert_eq!(classify_input(text), Err(NavigationError::TooLong));
        assert_eq!(NavigationUrl::parse(text), Err(NavigationError::TooLong));
    }
    assert_eq!(
        SearchConfig::default().search_url(&long_text),
        Err(NavigationError::TooLong)
    );
}

#[test]
fn input_exactly_at_the_cap_is_accepted_and_one_byte_over_is_not() {
    let prefix = "https://example.com/";
    let at_cap = format!("{prefix}{}", "a".repeat(MAX_INPUT_BYTES - prefix.len()));
    assert_eq!(at_cap.len(), MAX_INPUT_BYTES);
    assert!(classify_input(&at_cap).is_ok());
    assert_eq!(
        classify_input(&format!("{at_cap}a")),
        Err(NavigationError::TooLong)
    );
}

#[test]
fn a_huge_input_is_rejected_without_scanning_it() {
    let huge = "a".repeat(50 * 1024 * 1024);
    assert_eq!(classify_input(&huge), Err(NavigationError::TooLong));
}

#[test]
fn a_maximum_size_search_can_still_be_searched() {
    let query = "ä ".repeat(MAX_INPUT_BYTES / 3);
    assert!(query.len() <= MAX_INPUT_BYTES);
    let url = SearchConfig::default().search_url(&query).unwrap();
    assert!(url.as_str().starts_with("https://duckduckgo.com/?q="));
}

#[test]
fn a_very_long_label_or_many_labels_do_not_panic() {
    let long_label = format!("https://{}.com/", "a".repeat(5000));
    let _ = NavigationUrl::parse(&long_label);
    let many = format!("https://{}com/", "a.".repeat(3000));
    let _ = NavigationUrl::parse(&many);
}

#[test]
fn empty_and_whitespace_only_input_is_an_error_not_a_search() {
    for text in ["", " ", "\t\n\r", "\u{1}\u{2}", "\u{a0}"] {
        assert_eq!(
            classify_input(text),
            Err(NavigationError::Empty),
            "{text:?}"
        );
    }
}

// ---- Classification of ordinary input --------------------------------------------------------

#[test]
fn a_dotted_name_becomes_an_https_url() {
    assert_eq!(url_of("example.com").as_str(), "https://example.com/");
    assert_eq!(
        url_of("  example.com/a  ").as_str(),
        "https://example.com/a"
    );
    assert_eq!(
        url_of("Example.COM/Path?q=1#f").as_str(),
        "https://example.com/Path?q=1#f"
    );
}

#[test]
fn explicit_http_is_kept_and_flagged_insecure_while_bare_input_gets_https() {
    let http = url_of("http://example.com");
    assert!(http.is_insecure());
    assert_eq!(http.origin().scheme(), Scheme::Http);
    let bare = url_of("example.com");
    assert!(!bare.is_insecure());
    assert_eq!(bare.origin().scheme(), Scheme::Https);
}

#[test]
fn text_with_spaces_or_no_dot_is_searched_not_navigated() {
    assert_eq!(search_of("foo bar"), "foo bar");
    assert_eq!(search_of("what is rust?"), "what is rust?");
    assert_eq!(search_of("rust"), "rust");
    assert_eq!(search_of("rust?"), "rust?");
    assert_eq!(search_of("example.com is down"), "example.com is down");
    assert_eq!(search_of("  padded query  "), "padded query");
}

#[test]
fn a_search_that_merely_looks_like_a_url_with_a_path_but_no_host_is_searched() {
    assert_eq!(search_of("/etc/passwd"), "/etc/passwd");
    assert_eq!(search_of("\\\\server\\share"), "\\\\server\\share");
    assert_eq!(search_of("@someone"), "@someone");
    assert_eq!(search_of("foo/bar"), "foo/bar");
}

#[test]
fn unknown_scheme_looking_text_with_spaces_is_searched_not_dispatched() {
    assert_eq!(search_of("note: remember milk"), "note: remember milk");
}

#[test]
fn scheme_confusion_input_never_yields_a_non_web_url() {
    // Whatever these become, it must never be a URL that is not http(s).
    for text in [
        "https:evil.test",
        "https:/evil.test",
        "https:\\\\evil.test",
        "http:evil.test",
        "//evil.test",
        "HTTPS://EVIL.TEST",
    ] {
        if let Ok(Input::Url(u)) = classify_input(text) {
            assert!(matches!(u.as_url().scheme(), "http" | "https"), "{text}");
        }
    }
}

#[test]
fn idn_in_the_address_bar_is_converted_to_punycode_for_navigation() {
    let url = url_of("http://\u{4F8B}\u{3048}.jp/");
    assert!(url.as_str().starts_with("http://xn--"));
    assert!(url.origin().display_origin().shown_as_punycode);
}

// ---- Security state ------------------------------------------------------------------------------

#[test]
fn plain_http_can_never_be_displayed_as_secure_even_if_the_engine_says_so() {
    let http = NavigationUrl::parse("http://example.com/").unwrap();
    assert_eq!(
        SecurityState::resolve(&http, SecurityState::Secure),
        SecurityState::Insecure
    );
}

#[test]
fn an_engine_reported_certificate_error_is_never_downgraded_to_secure() {
    let https = NavigationUrl::parse("https://example.com/").unwrap();
    let http = NavigationUrl::parse("http://example.com/").unwrap();
    assert_eq!(
        SecurityState::resolve(&https, SecurityState::CertificateError),
        SecurityState::CertificateError
    );
    assert_eq!(
        SecurityState::resolve(&http, SecurityState::CertificateError),
        SecurityState::CertificateError
    );
    assert_eq!(
        SecurityState::resolve(&https, SecurityState::Secure),
        SecurityState::Secure
    );
}

// ---- Search encoding ---------------------------------------------------------------------------------

#[test]
fn search_text_is_percent_encoded() {
    let url = SearchConfig::default()
        .search_url("rust lang & more")
        .unwrap();
    assert_eq!(
        url.as_str(),
        "https://duckduckgo.com/?q=rust%20lang%20%26%20more"
    );
}

#[test]
fn search_text_cannot_inject_extra_parameters_fragments_or_paths() {
    let url = SearchConfig::default()
        .search_url("a&q=evil#frag/../x?y=z")
        .unwrap();
    assert_eq!(
        url.as_url().query(),
        Some("q=a%26q%3Devil%23frag%2F..%2Fx%3Fy%3Dz")
    );
    assert_eq!(url.as_url().fragment(), None);
    assert_eq!(url.as_url().path(), "/");
}

#[test]
fn search_text_cannot_change_the_host_of_the_search() {
    let cfg = SearchConfig::new("https://search.example/s/{query}").unwrap();
    let url = cfg.search_url("@evil.test/").unwrap();
    assert_eq!(url.origin().display_origin().to_string(), "search.example");
    assert_eq!(url.as_url().path(), "/s/%40evil.test%2F");
}

#[test]
fn search_text_with_control_characters_and_unicode_is_encoded() {
    let url = SearchConfig::default()
        .search_url("a\nb\0c \u{E9}\u{1F980}")
        .unwrap();
    assert_eq!(
        url.as_url().query(),
        Some("q=a%0Ab%00c%20%C3%A9%F0%9F%A6%80")
    );
}

#[test]
fn search_text_that_looks_like_javascript_is_inert_when_searched() {
    let url = SearchConfig::default()
        .search_url("javascript:alert(1)")
        .unwrap();
    assert_eq!(url.as_url().scheme(), "https");
    assert_eq!(url.as_url().query(), Some("q=javascript%3Aalert%281%29"));
}

#[test]
fn a_search_template_must_contain_the_placeholder_exactly_once() {
    for template in [
        "https://example.com/?q=",
        "https://example.com/?q={query}&r={query}",
        "https://example.com/?q={QUERY}",
        "",
    ] {
        assert!(
            matches!(
                SearchConfig::new(template),
                Err(NavigationError::InvalidSearchTemplate(_))
            ),
            "{template:?}"
        );
    }
}

#[test]
fn a_search_template_cannot_put_the_query_in_the_host() {
    for template in [
        "https://{query}.example.com/",
        "https://{query}",
        "https://example.com{query}",
        "https://user@{query}/",
        "{query}",
    ] {
        assert!(SearchConfig::new(template).is_err(), "{template:?}");
    }
}

#[test]
fn a_search_template_cannot_use_a_dangerous_scheme_or_credentials() {
    for template in [
        "javascript:alert({query})",
        "data:text/html,{query}",
        "file:///{query}",
        "https://user:pw@example.com/?q={query}",
        "mailto:x@example.com?subject={query}",
        "ftp://example.com/{query}",
    ] {
        assert!(SearchConfig::new(template).is_err(), "{template:?}");
    }
}

#[test]
fn the_default_search_provider_can_be_replaced() {
    let cfg = SearchConfig::new("https://example.org/find?term={query}&safe=1").unwrap();
    assert_eq!(
        cfg.search_url("a b").unwrap().as_str(),
        "https://example.org/find?term=a%20b&safe=1"
    );
}

// ---- Invariants under hostile input ---------------------------------------------------------------------

/// Whatever the input, an `Ok(Input::Url)` must satisfy every `NavigationUrl` invariant.
fn assert_invariants(text: &str) {
    match classify_input(text) {
        Ok(Input::Url(u)) => {
            let url = u.as_url();
            assert!(matches!(url.scheme(), "http" | "https"), "{text:?}");
            assert!(url.host().is_some(), "{text:?}");
            assert!(
                url.username().is_empty() && url.password().is_none(),
                "{text:?}"
            );
            assert_ne!(url.port(), Some(0), "{text:?}");
            assert!(!url.host_str().unwrap_or("").ends_with('.'), "{text:?}");
            // Re-validating a validated URL is a no-op.
            assert_eq!(NavigationUrl::parse(u.as_str()).unwrap(), u, "{text:?}");
        }
        Ok(Input::Search(s)) => assert!(!s.is_empty(), "{text:?}"),
        Ok(Input::External(link)) => {
            assert!(!matches!(
                link.scheme(),
                "http" | "https" | "javascript" | "data" | "file"
            ));
        }
        Err(_) => {}
    }
}

#[test]
fn a_corpus_of_hostile_strings_never_breaks_a_navigation_invariant() {
    let corpus = [
        "https://example.com:80:80/",
        "https://exa mple.com/",
        "https://example.com\u{0}/",
        "https://exa\u{200B}mple.com/",
        "https://\u{FF45}xample.com/",
        "https://example\u{3002}com/",
        "https://example.com%2e/",
        "https://%65xample.com/",
        "https://example.com/%00",
        "https://[::ffff:127.0.0.1]/",
        "https://1.1.1.1.1/",
        "https://0/",
        "http://\u{2028}example.com/",
        "https://example.com\\@evil.test",
        "https://evil.test\\.bank.com",
        "https:\t//\nexample.com",
        "hTTps://EXAMPLE.com",
        ":",
        "::",
        "://",
        "a:",
        "a://",
        "%",
        "%%%",
        "\u{202E}moc.elpmaxe",
        "exam\u{202E}ple.com",
        "example.com\u{0}",
        "\u{FEFF}example.com",
        "[",
        "]",
        "[]",
        "[::1",
        "@",
        "@@@@",
        "a@b@c.com",
        "https://a@b@c.com",
        "?",
        "#",
        ".",
        "..",
        "...",
        "-",
        "xn--",
        "xn--.com",
    ];
    for text in corpus {
        assert_invariants(text);
    }
}

#[test]
fn pseudo_random_token_soup_never_breaks_a_navigation_invariant() {
    const TOKENS: &[&str] = &[
        "http",
        "https",
        "javascript",
        "data",
        "file",
        ":",
        "//",
        "/",
        "\\",
        "@",
        ".",
        "..",
        "[",
        "]",
        "::1",
        "1",
        "0",
        "65535",
        "example",
        "com",
        "bank",
        "evil",
        " ",
        "\t",
        "\n",
        "%",
        "%40",
        "?",
        "#",
        "\u{430}",
        "\u{3BF}",
        "xn--",
        "\u{200B}",
        "\u{202E}",
        "\u{E9}",
        "mailto",
        "tel",
        "localhost",
        "-",
    ];
    // Small deterministic LCG so failures are reproducible without a rand dependency.
    let mut state: u64 = 0x2545_F491_4F6C_DD1D;
    let mut next = move || {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (state >> 33) as usize
    };
    for _ in 0..20_000 {
        let len = 1 + next() % 8;
        let text: String = (0..len).map(|_| TOKENS[next() % TOKENS.len()]).collect();
        assert_invariants(&text);
    }
}

// ---- Origin::from_parts: reconstruction must re-validate, never trust ----------------

fn parts_of(o: &Origin) -> (Scheme, String, u16) {
    (o.scheme(), o.host().to_string(), o.port())
}

#[test]
fn an_origin_rebuilt_from_its_own_parts_equals_the_original() {
    for site in [
        "https://app.example/",
        "http://plain.example:8080/x",
        "https://[::1]:9443/",
        "https://127.0.0.1/",
        "https://bücher.example/",
    ] {
        let o = NavigationUrl::parse(site).unwrap().origin();
        let (scheme, host, port) = parts_of(&o);
        assert_eq!(
            Origin::from_parts(scheme, &host, port).unwrap(),
            o,
            "{site}"
        );
    }
}

#[test]
fn stored_origin_parts_that_the_normal_constructor_would_reject_or_rewrite_are_refused() {
    for host in [
        "",
        "App.Example",
        "app.example.",
        "a..example",
        "user@app.example",
        "app.example/evil",
        "app.example?x",
        "app.example#x",
        "app.example:443",
        r"evil.example\@app.example",
        "bücher.example",
        "app example",
    ] {
        assert!(
            Origin::from_parts(Scheme::Https, host, 443).is_err(),
            "{host:?} must be refused"
        );
    }
    assert!(Origin::from_parts(Scheme::Https, "app.example", 0).is_err());
}

#[test]
fn from_parts_accepts_http_exactly_like_the_normal_constructor_and_keeps_it_distinct() {
    let http = Origin::from_parts(Scheme::Http, "a.example", 80).unwrap();
    let https = Origin::from_parts(Scheme::Https, "a.example", 443).unwrap();
    assert_eq!(
        http,
        NavigationUrl::parse("http://a.example/").unwrap().origin()
    );
    assert_ne!(http, https);
}
