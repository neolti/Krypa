//! Shared helpers: pagination and sanitizing of site-supplied text.

/// Largest page a list call returns.
pub const MAX_PAGE: usize = 200;

/// One page of a list query. `next` is the cursor for the following page, `None` at the
/// end.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Page<T, C> {
    /// Items of this page.
    pub items: Vec<T>,
    /// Pass to the same list call to get the next page.
    pub next: Option<C>,
}

/// Clamps a caller's limit to `1..=MAX_PAGE`.
pub(crate) fn clamp_limit(limit: usize) -> usize {
    limit.clamp(1, MAX_PAGE)
}

/// Splits `rows` (fetched with `limit + 1`) into a page, building the cursor from the last
/// kept item.
pub(crate) fn into_page<T, C>(
    mut rows: Vec<T>,
    limit: usize,
    cursor_of: impl Fn(&T) -> C,
) -> Page<T, C> {
    let more = rows.len() > limit;
    rows.truncate(limit);
    let next = if more {
        rows.last().map(cursor_of)
    } else {
        None
    };
    Page { items: rows, next }
}

/// Fetch size for a page: one extra row tells whether another page exists.
pub(crate) fn fetch_size(limit: usize) -> i64 {
    i64::try_from(limit + 1).unwrap_or(i64::MAX)
}

fn is_disallowed(c: char) -> bool {
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

/// Collapses whitespace runs to one space, drops control, bidi and invisible characters,
/// trims, and cuts to `max_chars` characters. May return an empty string.
pub(crate) fn sanitize_text(raw: &str, max_chars: usize) -> String {
    let mut out = String::new();
    let mut count = 0usize;
    let mut pending_space = false;
    for c in raw.chars() {
        if c.is_whitespace() {
            pending_space = count > 0;
            continue;
        }
        if is_disallowed(c) {
            continue;
        }
        let needed = if pending_space { 2 } else { 1 };
        if count + needed > max_chars {
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
