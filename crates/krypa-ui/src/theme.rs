//! Design tokens and the light and dark themes. Components read colors and sizes from
//! [`Tokens`] only; nothing else in the crate hard-codes a color.

use egui::{Color32, Context, CornerRadius, Stroke, Theme, ThemePreference, Visuals};

/// Smallest touch target edge, in points (Apple HIG: 44).
pub const MIN_TOUCH: f32 = 44.0;
/// Standard spacing unit.
pub const SPACE: f32 = 8.0;
/// Corner radius of cards and fields.
pub const RADIUS: u8 = 12;

/// What the user picked in settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ThemeChoice {
    /// Follow the operating system.
    #[default]
    System,
    /// Always light.
    Light,
    /// Always dark.
    Dark,
}

/// Colors of one theme. Text colors are meant for text on `bg` and `surface`; the
/// non-text colors (`border`, `success`, `warning`, `danger`, `accent`) keep at least 3:1
/// against both, checked by a test.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tokens {
    /// Screen background.
    pub bg: Color32,
    /// Cards, fields, toolbars.
    pub surface: Color32,
    /// Pressed or selected surface.
    pub surface_alt: Color32,
    /// Primary text.
    pub text: Color32,
    /// Secondary text.
    pub text_muted: Color32,
    /// Interactive accent.
    pub accent: Color32,
    /// Text on an `accent` fill.
    pub on_accent: Color32,
    /// Secure / live.
    pub success: Color32,
    /// Paused / caution.
    pub warning: Color32,
    /// Error / insecure.
    pub danger: Color32,
    /// Outlines of fields and cards.
    pub border: Color32,
}

impl Tokens {
    /// The light theme.
    #[must_use]
    pub const fn light() -> Self {
        Self {
            bg: Color32::from_rgb(0xF3, 0xF4, 0xF7),
            surface: Color32::from_rgb(0xFF, 0xFF, 0xFF),
            surface_alt: Color32::from_rgb(0xE4, 0xE7, 0xEE),
            text: Color32::from_rgb(0x14, 0x16, 0x1A),
            text_muted: Color32::from_rgb(0x4A, 0x51, 0x5F),
            accent: Color32::from_rgb(0x1A, 0x5C, 0xCF),
            on_accent: Color32::from_rgb(0xFF, 0xFF, 0xFF),
            success: Color32::from_rgb(0x1B, 0x73, 0x37),
            warning: Color32::from_rgb(0x8A, 0x55, 0x00),
            danger: Color32::from_rgb(0xB0, 0x24, 0x1C),
            border: Color32::from_rgb(0x6B, 0x72, 0x80),
        }
    }

    /// The dark theme.
    #[must_use]
    pub const fn dark() -> Self {
        Self {
            bg: Color32::from_rgb(0x0E, 0x10, 0x14),
            surface: Color32::from_rgb(0x1A, 0x1D, 0x24),
            surface_alt: Color32::from_rgb(0x2A, 0x2F, 0x3A),
            text: Color32::from_rgb(0xEC, 0xEE, 0xF3),
            text_muted: Color32::from_rgb(0xA9, 0xB0, 0xBF),
            accent: Color32::from_rgb(0x8A, 0xB4, 0xFF),
            on_accent: Color32::from_rgb(0x08, 0x16, 0x33),
            success: Color32::from_rgb(0x6F, 0xD9, 0x92),
            warning: Color32::from_rgb(0xFF, 0xC2, 0x66),
            danger: Color32::from_rgb(0xFF, 0x8A, 0x80),
            border: Color32::from_rgb(0x80, 0x88, 0x99),
        }
    }

    /// Tokens of an egui theme.
    #[must_use]
    pub const fn for_theme(theme: Theme) -> Self {
        match theme {
            Theme::Light => Self::light(),
            Theme::Dark => Self::dark(),
        }
    }

    /// Builds egui visuals from the tokens.
    #[must_use]
    pub fn visuals(&self, theme: Theme) -> Visuals {
        let mut v = match theme {
            Theme::Light => Visuals::light(),
            Theme::Dark => Visuals::dark(),
        };
        let radius = CornerRadius::same(RADIUS);
        v.override_text_color = Some(self.text);
        v.panel_fill = self.bg;
        v.window_fill = self.surface;
        v.extreme_bg_color = self.surface;
        v.faint_bg_color = self.surface_alt;
        v.hyperlink_color = self.accent;
        v.window_corner_radius = radius;
        v.window_stroke = Stroke::new(1.0, self.border);
        v.selection.bg_fill = self.accent;
        v.selection.stroke = Stroke::new(2.0, self.accent);
        let w = &mut v.widgets;
        for (state, fill) in [
            (&mut w.noninteractive, self.bg),
            (&mut w.inactive, self.surface),
            (&mut w.hovered, self.surface_alt),
            (&mut w.active, self.surface_alt),
            (&mut w.open, self.surface_alt),
        ] {
            state.bg_fill = fill;
            state.weak_bg_fill = fill;
            state.bg_stroke = Stroke::new(1.0, self.border);
            state.fg_stroke = Stroke::new(1.5, self.text);
            state.corner_radius = radius;
        }
        v
    }
}

/// Installs the visuals and spacing of both themes and selects the one in use. Cheap to
/// call every frame: it only touches the context when something changed.
pub(crate) fn apply(ctx: &Context, choice: ThemeChoice, reduced_motion: bool) {
    let applied = ctx.data(|d| d.get_temp::<(ThemeChoice, bool)>(egui::Id::unique("krypa-theme")));
    if applied == Some((choice, reduced_motion)) {
        return;
    }
    ctx.set_visuals_of(Theme::Light, Tokens::light().visuals(Theme::Light));
    ctx.set_visuals_of(Theme::Dark, Tokens::dark().visuals(Theme::Dark));
    ctx.set_theme(match choice {
        ThemeChoice::System => ThemePreference::System,
        ThemeChoice::Light => ThemePreference::Light,
        ThemeChoice::Dark => ThemePreference::Dark,
    });
    ctx.all_styles_mut(|style| {
        style.spacing.interact_size.y = MIN_TOUCH;
        style.spacing.interact_size.x = MIN_TOUCH;
        style.spacing.item_spacing = egui::vec2(SPACE, SPACE);
        style.spacing.button_padding = egui::vec2(14.0, 10.0);
        style.spacing.icon_width = 28.0;
        style.spacing.icon_spacing = 10.0;
        style.animation_time = if reduced_motion { 0.0 } else { 1.0 / 12.0 };
    });
    ctx.data_mut(|d| d.insert_temp(egui::Id::unique("krypa-theme"), (choice, reduced_motion)));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn channel(c: u8) -> f64 {
        let s = f64::from(c) / 255.0;
        if s <= 0.040_45 {
            s / 12.92
        } else {
            ((s + 0.055) / 1.055).powf(2.4)
        }
    }

    fn luminance(c: Color32) -> f64 {
        0.2126 * channel(c.r()) + 0.7152 * channel(c.g()) + 0.0722 * channel(c.b())
    }

    fn contrast(a: Color32, b: Color32) -> f64 {
        let (la, lb) = (luminance(a), luminance(b));
        let (hi, lo) = if la > lb { (la, lb) } else { (lb, la) };
        (hi + 0.05) / (lo + 0.05)
    }

    #[test]
    fn contrast_helper_matches_known_values() {
        let white = Color32::WHITE;
        let black = Color32::BLACK;
        assert!((contrast(white, black) - 21.0).abs() < 0.01);
        assert!((contrast(white, white) - 1.0).abs() < 0.01);
    }

    /// Why: WCAG 2.1 AA requires 4.5:1 for normal text and 3:1 for non-text UI parts. A
    /// theme tweak that drops below must fail here, not in a user's hands.
    #[test]
    fn both_themes_meet_wcag_aa() {
        for (name, t) in [("light", Tokens::light()), ("dark", Tokens::dark())] {
            for (bg_name, bg) in [("bg", t.bg), ("surface", t.surface), ("alt", t.surface_alt)] {
                for (fg_name, fg) in [
                    ("text", t.text),
                    ("text_muted", t.text_muted),
                    ("success", t.success),
                    ("warning", t.warning),
                    ("danger", t.danger),
                    ("accent", t.accent),
                ] {
                    let ratio = contrast(fg, bg);
                    assert!(ratio >= 4.5, "{name}: {fg_name} on {bg_name} is {ratio:.2}");
                }
            }
            let ratio = contrast(t.on_accent, t.accent);
            assert!(ratio >= 4.5, "{name}: on_accent on accent is {ratio:.2}");
            for (bg_name, bg) in [("bg", t.bg), ("surface", t.surface)] {
                let ratio = contrast(t.border, bg);
                assert!(ratio >= 3.0, "{name}: border on {bg_name} is {ratio:.2}");
            }
        }
    }
}
