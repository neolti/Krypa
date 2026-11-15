/// An 8-bit RGBA colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgba {
    /// Red.
    pub r: u8,
    /// Green.
    pub g: u8,
    /// Blue.
    pub b: u8,
    /// Alpha, 255 is opaque.
    pub a: u8,
}

impl Rgba {
    /// Parses a manifest colour. Supported: `#rgb`, `#rgba`, `#rrggbb`, `#rrggbbaa`,
    /// comma-separated `rgb()`/`rgba()` with integer channels and a 0..=1 alpha, and the
    /// names `black`, `white`, `red`, `green`, `blue`, `transparent`. Anything else,
    /// including out-of-range values, returns `None` so the caller drops the colour.
    #[must_use]
    pub fn parse(input: &str) -> Option<Self> {
        let s = input.trim().to_ascii_lowercase();
        if s.len() > 64 {
            return None;
        }
        if let Some(hex) = s.strip_prefix('#') {
            return parse_hex(hex);
        }
        if let Some(args) = s.strip_prefix("rgba(").or_else(|| s.strip_prefix("rgb(")) {
            return parse_functional(args.strip_suffix(')')?);
        }
        let (r, g, b, a) = match s.as_str() {
            "black" => (0, 0, 0, 255),
            "white" => (255, 255, 255, 255),
            "red" => (255, 0, 0, 255),
            "green" => (0, 128, 0, 255),
            "blue" => (0, 0, 255, 255),
            "transparent" => (0, 0, 0, 0),
            _ => return None,
        };
        Some(Self { r, g, b, a })
    }
}

fn parse_hex(hex: &str) -> Option<Rgba> {
    if !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let digits: Vec<u8> = hex
        .chars()
        .filter_map(|c| c.to_digit(16))
        .filter_map(|d| u8::try_from(d).ok())
        .collect();
    let channels: Vec<u8> = match digits.len() {
        3 | 4 => digits.iter().map(|d| d * 17).collect(),
        6 | 8 => digits.chunks(2).map(|p| p[0] * 16 + p[1]).collect(),
        _ => return None,
    };
    Some(Rgba {
        r: channels[0],
        g: channels[1],
        b: channels[2],
        a: channels.get(3).copied().unwrap_or(255),
    })
}

fn parse_functional(args: &str) -> Option<Rgba> {
    let parts: Vec<&str> = args.split(',').map(str::trim).collect();
    if parts.len() != 3 && parts.len() != 4 {
        return None;
    }
    let r = parts[0].parse::<u8>().ok()?;
    let g = parts[1].parse::<u8>().ok()?;
    let b = parts[2].parse::<u8>().ok()?;
    let a = match parts.get(3) {
        None => 255,
        Some(p) => {
            let alpha = p.parse::<f32>().ok()?;
            if !(0.0..=1.0).contains(&alpha) {
                return None;
            }
            (alpha * 255.0).round() as u8
        }
    };
    Some(Rgba { r, g, b, a })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rgba(r: u8, g: u8, b: u8, a: u8) -> Option<Rgba> {
        Some(Rgba { r, g, b, a })
    }

    #[test]
    fn valid_colours_parse_to_the_expected_rgba() {
        assert_eq!(Rgba::parse("#fff"), rgba(255, 255, 255, 255));
        assert_eq!(Rgba::parse("#1a2B3c"), rgba(0x1a, 0x2b, 0x3c, 255));
        assert_eq!(Rgba::parse("#0f08"), rgba(0, 255, 0, 0x88));
        assert_eq!(Rgba::parse("  #11223344 "), rgba(0x11, 0x22, 0x33, 0x44));
        assert_eq!(Rgba::parse("rgb(1, 2, 3)"), rgba(1, 2, 3, 255));
        assert_eq!(Rgba::parse("RGBA(1,2,3,0.5)"), rgba(1, 2, 3, 128));
        assert_eq!(Rgba::parse("transparent"), rgba(0, 0, 0, 0));
        assert_eq!(Rgba::parse("Green"), rgba(0, 128, 0, 255));
    }

    #[test]
    fn invalid_colours_are_dropped_not_guessed() {
        for bad in [
            "",
            "#",
            "#ff",
            "#fffff",
            "#ggg",
            "#12345678a",
            "#\u{ff26}\u{ff26}\u{ff26}",
            "rgb(256,0,0)",
            "rgb(-1,0,0)",
            "rgb(1,2)",
            "rgb(1,2,3,1.5)",
            "rgb(1,2,3,NaN)",
            "rgb(1,2,3",
            "rgb(1 2 3)",
            "javascript:alert(1)",
            "url(x)",
            "notacolor",
            "red; x",
        ] {
            assert_eq!(Rgba::parse(bad), None, "input {bad:?}");
        }
        assert_eq!(Rgba::parse(&format!("#{}", "f".repeat(10_000))), None);
    }
}
