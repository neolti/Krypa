//! Size, scale and safe-area maths. The shell reports the view in physical pixels (what the
//! Metal drawable needs) plus the scale factor and the insets in points (what egui needs);
//! everything is checked here, once, so no later stage divides by zero or draws NaN.

use egui::{pos2, vec2, Rect};

/// Largest drawable edge accepted, in pixels. Metal's own limit is 16384 on current GPUs; a
/// bigger request is a bug in the shell, not something to hand to the GPU.
pub const MAX_EDGE_PX: u32 = 16_384;
/// Largest scale factor accepted. iPhones are at most 3.0; 8.0 leaves room and still rejects
/// garbage.
pub const MAX_SCALE: f32 = 8.0;

/// Why the shell's numbers were refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GeometryError {
    /// A pixel edge was 0 or above [`MAX_EDGE_PX`].
    BadSize,
    /// The scale was NaN, infinite, not positive or above [`MAX_SCALE`].
    BadScale,
    /// An inset was NaN, infinite or negative, or the insets leave no room (they must stay
    /// below the size in points on each axis).
    BadInsets,
}

/// Space the system keeps for itself, in points (`UIView.safeAreaInsets`).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Insets {
    /// Top (notch, Dynamic Island, status bar).
    pub top: f32,
    /// Left (landscape notch).
    pub left: f32,
    /// Bottom (home indicator).
    pub bottom: f32,
    /// Right (landscape notch).
    pub right: f32,
}

/// A validated view geometry.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Viewport {
    width_px: u32,
    height_px: u32,
    scale: f32,
    insets: Insets,
}

impl Viewport {
    /// Validates the shell's numbers.
    ///
    /// # Errors
    /// See [`GeometryError`].
    pub fn new(
        width_px: u32,
        height_px: u32,
        scale: f32,
        insets: Insets,
    ) -> Result<Self, GeometryError> {
        if width_px == 0 || height_px == 0 || width_px > MAX_EDGE_PX || height_px > MAX_EDGE_PX {
            return Err(GeometryError::BadSize);
        }
        if !scale.is_finite() || scale <= 0.0 || scale > MAX_SCALE {
            return Err(GeometryError::BadScale);
        }
        let sides = [insets.top, insets.left, insets.bottom, insets.right];
        if sides.iter().any(|v| !v.is_finite() || *v < 0.0) {
            return Err(GeometryError::BadInsets);
        }
        let (w, h) = (width_px as f32 / scale, height_px as f32 / scale);
        if insets.left + insets.right >= w || insets.top + insets.bottom >= h {
            return Err(GeometryError::BadInsets);
        }
        Ok(Self {
            width_px,
            height_px,
            scale,
            insets,
        })
    }

    /// Drawable size in pixels.
    #[must_use]
    pub fn size_px(&self) -> [u32; 2] {
        [self.width_px, self.height_px]
    }

    /// Pixels per point.
    #[must_use]
    pub fn scale(&self) -> f32 {
        self.scale
    }

    /// The whole view in points: what egui calls the screen rectangle.
    #[must_use]
    pub fn screen_rect(&self) -> Rect {
        Rect::from_min_size(
            pos2(0.0, 0.0),
            vec2(
                self.width_px as f32 / self.scale,
                self.height_px as f32 / self.scale,
            ),
        )
    }

    /// The insets in egui's type, for `RawInput::safe_area_insets`.
    #[must_use]
    pub fn safe_area_insets(&self) -> egui::SafeAreaInsets {
        egui::SafeAreaInsets(egui::epaint::MarginF32 {
            left: self.insets.left,
            right: self.insets.right,
            top: self.insets.top,
            bottom: self.insets.bottom,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NO_INSETS: Insets = Insets {
        top: 0.0,
        left: 0.0,
        bottom: 0.0,
        right: 0.0,
    };

    #[test]
    fn iphone_geometry_converts_pixels_to_points() {
        // iPhone 15: 1179 x 2556 px at 3x is 393 x 852 points. egui lays out in points, so
        // getting this wrong shows up as a UI that is 3x too big or too small.
        let insets = Insets {
            top: 59.0,
            bottom: 34.0,
            ..NO_INSETS
        };
        let vp = Viewport::new(1179, 2556, 3.0, insets).unwrap();
        assert_eq!(vp.screen_rect().size(), vec2(393.0, 852.0));
        assert_eq!(vp.size_px(), [1179, 2556]);
    }

    #[test]
    fn safe_area_insets_reach_egui_unchanged() {
        // The launcher keeps clear of the notch and the home indicator only through these.
        let insets = Insets {
            top: 59.0,
            left: 1.0,
            bottom: 34.0,
            right: 2.0,
        };
        let vp = Viewport::new(1179, 2556, 3.0, insets).unwrap();
        let egui::SafeAreaInsets(m) = vp.safe_area_insets();
        assert_eq!((m.top, m.left, m.bottom, m.right), (59.0, 1.0, 34.0, 2.0));
    }

    #[test]
    fn zero_or_huge_sizes_are_refused() {
        for (w, h) in [(0, 100), (100, 0), (MAX_EDGE_PX + 1, 100), (100, u32::MAX)] {
            assert_eq!(
                Viewport::new(w, h, 2.0, NO_INSETS),
                Err(GeometryError::BadSize)
            );
        }
    }

    #[test]
    fn bad_scales_are_refused_so_nothing_divides_by_zero() {
        for s in [0.0, -1.0, f32::NAN, f32::INFINITY, MAX_SCALE + 0.1] {
            assert_eq!(
                Viewport::new(100, 100, s, NO_INSETS),
                Err(GeometryError::BadScale)
            );
        }
    }

    #[test]
    fn bad_insets_are_refused() {
        let bad = [
            Insets {
                top: -1.0,
                ..NO_INSETS
            },
            Insets {
                left: f32::NAN,
                ..NO_INSETS
            },
            Insets {
                bottom: f32::INFINITY,
                ..NO_INSETS
            },
            // 100 px at 2x is 50 points; insets that eat all of it leave nothing to draw in.
            Insets {
                top: 30.0,
                bottom: 20.0,
                ..NO_INSETS
            },
            Insets {
                left: 25.0,
                right: 25.0,
                ..NO_INSETS
            },
        ];
        for insets in bad {
            assert_eq!(
                Viewport::new(100, 100, 2.0, insets),
                Err(GeometryError::BadInsets)
            );
        }
    }
}
