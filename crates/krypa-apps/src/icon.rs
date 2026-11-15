use url::Url;

use crate::app::is_plain_https_url;
use crate::manifest::ManifestIcon;

/// Largest number of `sizes` tokens read per icon.
pub(crate) const MAX_SIZE_TOKENS: usize = 16;

/// One entry of an icon's `sizes`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IconSize {
    /// `any`: scalable, for example SVG.
    Any,
    /// A raster size in pixels.
    Px {
        /// Width in pixels.
        width: u32,
        /// Height in pixels.
        height: u32,
    },
}

/// What an icon may be used for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IconPurpose {
    /// A normal icon.
    Any,
    /// Designed for adaptive masks.
    Maskable,
    /// A single-colour glyph.
    Monochrome,
}

/// The set of purposes an icon declares. Never empty for a validated icon.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IconPurposes {
    /// Usable as [`IconPurpose::Any`].
    pub any: bool,
    /// Usable as [`IconPurpose::Maskable`].
    pub maskable: bool,
    /// Usable as [`IconPurpose::Monochrome`].
    pub monochrome: bool,
}

impl IconPurposes {
    /// Whether the set contains `purpose`.
    #[must_use]
    pub fn contains(self, purpose: IconPurpose) -> bool {
        match purpose {
            IconPurpose::Any => self.any,
            IconPurpose::Maskable => self.maskable,
            IconPurpose::Monochrome => self.monochrome,
        }
    }
}

/// A validated icon reference. The URL is always `https` with a host and no credentials.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Icon {
    pub(crate) src: Url,
    pub(crate) sizes: Vec<IconSize>,
    pub(crate) purposes: IconPurposes,
}

impl Icon {
    /// The absolute icon URL (`https` only).
    #[must_use]
    pub fn src(&self) -> &Url {
        &self.src
    }

    /// The declared sizes; may be empty if none were valid.
    #[must_use]
    pub fn sizes(&self) -> &[IconSize] {
        &self.sizes
    }

    /// The declared purposes.
    #[must_use]
    pub fn purposes(&self) -> IconPurposes {
        self.purposes
    }

    /// Validates a raw entry, resolving `src` against the manifest URL. Returns `None`
    /// for a missing/unparseable `src`, a non-`https` URL (so `javascript:`, `data:` and
    /// plain `http:` are all dropped), credentials, or a `purpose` with no known token.
    pub(crate) fn from_raw(manifest_url: &Url, raw: &ManifestIcon) -> Option<Self> {
        let src = manifest_url.join(raw.src.as_deref()?.trim()).ok()?;
        if !is_plain_https_url(&src) {
            return None;
        }
        Some(Self {
            src,
            sizes: parse_sizes(raw.sizes.as_deref().unwrap_or("")),
            purposes: parse_purposes(raw.purpose.as_deref())?,
        })
    }
}

fn parse_sizes(text: &str) -> Vec<IconSize> {
    text.split_ascii_whitespace()
        .take(MAX_SIZE_TOKENS)
        .filter_map(|token| {
            if token.eq_ignore_ascii_case("any") {
                return Some(IconSize::Any);
            }
            let (w, h) = token.split_once(['x', 'X'])?;
            Some(IconSize::Px {
                width: parse_dimension(w)?,
                height: parse_dimension(h)?,
            })
        })
        .collect()
}

fn parse_dimension(text: &str) -> Option<u32> {
    if text.is_empty() || text.starts_with('0') || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

fn parse_purposes(text: Option<&str>) -> Option<IconPurposes> {
    let mut p = IconPurposes {
        any: false,
        maskable: false,
        monochrome: false,
    };
    let mut tokens = 0;
    for token in text.unwrap_or("").split_ascii_whitespace() {
        tokens += 1;
        match token.to_ascii_lowercase().as_str() {
            "any" => p.any = true,
            "maskable" => p.maskable = true,
            "monochrome" => p.monochrome = true,
            _ => {}
        }
    }
    if tokens == 0 {
        p.any = true;
    }
    (p.any || p.maskable || p.monochrome).then_some(p)
}

/// Picks the icon with `purpose` whose size best fits `target_px`: a scalable (`any`)
/// icon or an exact match first, then the smallest raster larger than `target_px`
/// (downscaling stays sharp), then the largest smaller one, then icons with no valid
/// size. Ties go to the earlier manifest entry.
pub(crate) fn select_best(icons: &[Icon], target_px: u32, purpose: IconPurpose) -> Option<&Icon> {
    icons
        .iter()
        .filter(|i| i.purposes.contains(purpose))
        .min_by_key(|i| rank(i, target_px))
}

fn rank(icon: &Icon, target: u32) -> (u8, u32) {
    icon.sizes
        .iter()
        .map(|s| match s {
            IconSize::Any => (0, 0),
            IconSize::Px { width, height } => {
                let side = (*width).min(*height);
                if side >= target {
                    (0, side - target)
                } else {
                    (1, target - side)
                }
            }
        })
        .min()
        .unwrap_or((2, 0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn size_tokens_follow_the_spec_grammar() {
        assert_eq!(
            parse_sizes("48x48 ANY 192X192"),
            vec![
                IconSize::Px {
                    width: 48,
                    height: 48
                },
                IconSize::Any,
                IconSize::Px {
                    width: 192,
                    height: 192
                },
            ]
        );
        assert!(parse_sizes("0x0 048x48 -1x5 axb 5 x5 99999999999x1 48x48x48").is_empty());
    }

    #[test]
    fn oversized_size_lists_are_capped() {
        assert_eq!(parse_sizes(&"16x16 ".repeat(1000)).len(), MAX_SIZE_TOKENS);
    }

    #[test]
    fn purposes_default_to_any_and_ignore_unknown_tokens() {
        assert!(parse_purposes(None).unwrap().any);
        assert!(parse_purposes(Some("  ")).unwrap().any);
        let p = parse_purposes(Some("Maskable bogus")).unwrap();
        assert!(p.maskable && !p.any);
        assert_eq!(parse_purposes(Some("bogus")), None);
    }
}
