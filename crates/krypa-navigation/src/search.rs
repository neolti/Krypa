use crate::error::{NavigationError, MAX_INPUT_BYTES};
use crate::navigation_url::NavigationUrl;

const PLACEHOLDER: &str = "{query}";

/// A search provider: a URL template containing exactly one `{query}`.
///
/// The placeholder must be in the path or query, never in the host, so query
/// text can not redirect a search to another site. The query is percent-encoded
/// (everything except `A-Z a-z 0-9 - . _ ~`), so it can not break out of its
/// parameter by injecting `&`, `#`, or `/`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchConfig {
    prefix: String,
    suffix: String,
}

impl SearchConfig {
    /// Validates `template`, e.g. `https://example.com/search?q={query}`.
    pub fn new(template: &str) -> Result<Self, NavigationError> {
        if template.matches(PLACEHOLDER).count() != 1 {
            return Err(NavigationError::InvalidSearchTemplate(
                "template must contain {query} exactly once",
            ));
        }
        let Some((prefix, suffix)) = template.split_once(PLACEHOLDER) else {
            return Err(NavigationError::InvalidSearchTemplate("missing {query}"));
        };
        let authority_start = prefix.find("://").map_or(0, |i| i + 3);
        if !prefix[authority_start..].contains(['/', '?', '#']) {
            return Err(NavigationError::InvalidSearchTemplate(
                "{query} must be in the path or query, not the host",
            ));
        }
        let config = Self {
            prefix: prefix.to_owned(),
            suffix: suffix.to_owned(),
        };
        // Fails on non-web schemes, credentials, bad hosts: a bad template is never usable.
        config.search_url("x")?;
        Ok(config)
    }

    /// Builds the navigation for a search `query`.
    pub fn search_url(&self, query: &str) -> Result<NavigationUrl, NavigationError> {
        if query.len() > MAX_INPUT_BYTES {
            return Err(NavigationError::TooLong);
        }
        let mut text = String::with_capacity(self.prefix.len() + self.suffix.len() + query.len());
        text.push_str(&self.prefix);
        percent_encode_into(query, &mut text);
        text.push_str(&self.suffix);
        // The encoded query can be three times its raw size.
        NavigationUrl::parse_limited(&text, MAX_INPUT_BYTES * 4)
    }
}

impl Default for SearchConfig {
    /// DuckDuckGo (`https://duckduckgo.com/?q={query}`). The default provider is
    /// a product decision; override with [`SearchConfig::new`].
    fn default() -> Self {
        Self {
            prefix: "https://duckduckgo.com/?q=".to_owned(),
            suffix: String::new(),
        }
    }
}

fn percent_encode_into(query: &str, out: &mut String) {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    for &b in query.as_bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(char::from(b));
        } else {
            out.push('%');
            out.push(char::from(HEX[usize::from(b >> 4)]));
            out.push(char::from(HEX[usize::from(b & 0x0F)]));
        }
    }
}
