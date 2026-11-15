//! Vector icons drawn with the painter, so the crate needs no icon font or image loader.
//! Icons are decoration: every button that uses one also carries a text label for
//! assistive technology.

use egui::{pos2, vec2, Color32, Painter, Pos2, Rect, Shape, Stroke};

/// The icons the interface uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    /// Chevron pointing left.
    Back,
    /// Chevron pointing right.
    Forward,
    /// Circular arrow.
    Reload,
    /// Cross, used for stop and close.
    Cross,
    /// House.
    Home,
    /// Two overlapping pages.
    Pages,
    /// Plus sign.
    Plus,
    /// Gear-like slider glyph.
    Settings,
    /// Closed padlock.
    Lock,
    /// Triangle with an exclamation mark.
    Warning,
    /// Two vertical bars.
    Pause,
    /// Hollow circle: nothing confirmed.
    Unverified,
}

fn line(p: &Painter, pts: &[Pos2], stroke: Stroke) {
    p.add(Shape::line(pts.to_vec(), stroke));
}

/// Paints `icon` centered in `rect`, scaled to fit.
pub fn paint_icon(p: &Painter, rect: Rect, icon: Icon, color: Color32) {
    let c = rect.center();
    let r = rect.width().min(rect.height()) * 0.5;
    let s = Stroke::new((r * 0.16).max(1.5), color);
    let at = |x: f32, y: f32| pos2(c.x + x * r, c.y + y * r);
    match icon {
        Icon::Back => line(p, &[at(0.25, -0.6), at(-0.3, 0.0), at(0.25, 0.6)], s),
        Icon::Forward => line(p, &[at(-0.25, -0.6), at(0.3, 0.0), at(-0.25, 0.6)], s),
        Icon::Reload => {
            let pts: Vec<Pos2> = (0..=18)
                .map(|i| {
                    let a = std::f32::consts::TAU * (0.12 + 0.76 * i as f32 / 18.0);
                    at(0.6 * a.cos(), 0.6 * a.sin())
                })
                .collect();
            p.add(Shape::line(pts, s));
            line(p, &[at(0.75, -0.55), at(0.6, -0.05), at(0.1, -0.2)], s);
        }
        Icon::Cross => {
            line(p, &[at(-0.5, -0.5), at(0.5, 0.5)], s);
            line(p, &[at(-0.5, 0.5), at(0.5, -0.5)], s);
        }
        Icon::Home => {
            line(p, &[at(-0.7, 0.0), at(0.0, -0.65), at(0.7, 0.0)], s);
            line(
                p,
                &[at(-0.5, -0.1), at(-0.5, 0.6), at(0.5, 0.6), at(0.5, -0.1)],
                s,
            );
        }
        Icon::Pages => {
            p.rect_stroke(
                Rect::from_center_size(at(-0.12, 0.12), vec2(1.0, 1.0) * r),
                2.0,
                s,
                egui::StrokeKind::Middle,
            );
            line(p, &[at(-0.1, -0.55), at(0.55, -0.55), at(0.55, 0.1)], s);
        }
        Icon::Plus => {
            line(p, &[at(-0.55, 0.0), at(0.55, 0.0)], s);
            line(p, &[at(0.0, -0.55), at(0.0, 0.55)], s);
        }
        Icon::Settings => {
            for (y, x) in [(-0.45, -0.25), (0.0, 0.3), (0.45, -0.1)] {
                line(p, &[at(-0.6, y), at(0.6, y)], s);
                p.circle_filled(at(x, y), r * 0.17, color);
            }
        }
        Icon::Lock => {
            p.rect_stroke(
                Rect::from_center_size(at(0.0, 0.2), vec2(1.1, 0.8) * r),
                2.0,
                s,
                egui::StrokeKind::Middle,
            );
            line(
                p,
                &[at(-0.3, -0.2), at(-0.3, -0.5), at(0.3, -0.5), at(0.3, -0.2)],
                s,
            );
        }
        Icon::Warning => {
            line(
                p,
                &[at(0.0, -0.7), at(0.7, 0.55), at(-0.7, 0.55), at(0.0, -0.7)],
                s,
            );
            line(p, &[at(0.0, -0.2), at(0.0, 0.15)], s);
            p.circle_filled(at(0.0, 0.38), r * 0.08, color);
        }
        Icon::Unverified => {
            p.circle_stroke(at(0.0, 0.0), r * 0.6, s);
        }
        Icon::Pause => {
            line(p, &[at(-0.25, -0.5), at(-0.25, 0.5)], s);
            line(p, &[at(0.25, -0.5), at(0.25, 0.5)], s);
        }
    }
}
