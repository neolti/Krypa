//! Rendering of web-derived text (titles, app names, origins). It is hostile: it is only
//! ever drawn as plain text (egui does not interpret markup in labels), stripped of
//! control and bidi-control characters, and cut to a fixed length.

use std::sync::Arc;

use egui::text::{LayoutJob, TextFormat, TextWrapping};
use egui::{Color32, FontId, Galley, Ui};

/// Longest title or name shown anywhere, in characters.
pub const MAX_DISPLAY_CHARS: usize = 60;

fn is_unsafe_char(c: char) -> bool {
    c.is_control()
        || matches!(
            c,
            '\u{061C}'
                | '\u{200B}'..='\u{200F}'
                | '\u{202A}'..='\u{202E}'
                | '\u{2060}'..='\u{2069}'
                | '\u{FEFF}'
        )
}

/// Removes control and bidi-control characters, collapses whitespace runs, and truncates
/// to `max_chars` characters with an ellipsis. Never fails; empty in, empty out.
#[must_use]
pub fn display_text(raw: &str, max_chars: usize) -> String {
    let cleaned: String = raw
        .chars()
        .map(|c| if c.is_whitespace() { ' ' } else { c })
        .filter(|c| !is_unsafe_char(*c))
        .collect();
    let collapsed = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() <= max_chars {
        return collapsed;
    }
    let keep = max_chars.saturating_sub(1);
    let mut out: String = collapsed.chars().take(keep).collect();
    out.push('\u{2026}');
    out
}

/// Lays text out on one row of at most `width` points, cut with an ellipsis if longer.
pub(crate) fn one_line(
    ui: &Ui,
    text: &str,
    font: FontId,
    color: Color32,
    width: f32,
) -> Arc<Galley> {
    let job = LayoutJob {
        wrap: TextWrapping {
            max_width: width.max(1.0),
            max_rows: 1,
            break_anywhere: true,
            overflow_character: Some('\u{2026}'),
        },
        ..LayoutJob::single_section(
            text.to_owned(),
            TextFormat {
                font_id: font,
                color,
                ..TextFormat::default()
            },
        )
    };
    ui.fonts_mut(|f| f.layout_job(job))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_bidi_and_control_characters() {
        // A right-to-left override can make `evil.com` read as `moc.live`.
        assert_eq!(
            display_text("pay\u{202E}moc.live\u{202C}", 60),
            "paymoc.live"
        );
        assert_eq!(display_text("a\u{0}b\nc", 60), "ab c");
    }

    #[test]
    fn truncates_long_text_by_characters_not_bytes() {
        let long = "\u{00E9}".repeat(200);
        let out = display_text(&long, 10);
        assert_eq!(out.chars().count(), 10);
        assert!(out.ends_with('\u{2026}'));
    }

    #[test]
    fn markup_is_left_as_literal_text() {
        assert_eq!(display_text("<b>**hi**</b>", 60), "<b>**hi**</b>");
    }
}
