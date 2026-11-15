use serde_json::{Map, Value};

use crate::error::ManifestError;

/// Largest manifest text accepted, in bytes. Larger input is rejected unparsed.
pub const MAX_MANIFEST_BYTES: usize = 256 * 1024;

/// Largest number of `icons` entries read; the rest are ignored.
pub const MAX_ICONS: usize = 64;

/// The manifest members Krypa reads, exactly as written by the site.
///
/// Every member whose JSON type is wrong is `None`. Nothing here is validated yet;
/// pass it to [`crate::InstalledApp::from_manifest`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Manifest {
    /// `name`.
    pub name: Option<String>,
    /// `short_name`.
    pub short_name: Option<String>,
    /// `id`.
    pub id: Option<String>,
    /// `start_url`.
    pub start_url: Option<String>,
    /// `scope`.
    pub scope: Option<String>,
    /// `display`.
    pub display: Option<String>,
    /// `theme_color`.
    pub theme_color: Option<String>,
    /// `background_color`.
    pub background_color: Option<String>,
    /// `icons`, at most [`MAX_ICONS`] object entries.
    pub icons: Vec<ManifestIcon>,
}

/// One raw `icons` entry.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ManifestIcon {
    /// `src`.
    pub src: Option<String>,
    /// `sizes`.
    pub sizes: Option<String>,
    /// `purpose`.
    pub purpose: Option<String>,
}

impl Manifest {
    /// Parses manifest JSON.
    ///
    /// Unknown members are ignored and members of the wrong type become absent.
    ///
    /// # Errors
    /// [`ManifestError::TooLarge`] above [`MAX_MANIFEST_BYTES`],
    /// [`ManifestError::InvalidJson`] for malformed or too deeply nested JSON (the JSON
    /// parser enforces a recursion limit), [`ManifestError::NotAnObject`] otherwise.
    pub fn parse(text: &str) -> Result<Self, ManifestError> {
        if text.len() > MAX_MANIFEST_BYTES {
            return Err(ManifestError::TooLarge {
                len: text.len(),
                max: MAX_MANIFEST_BYTES,
            });
        }
        let text = text.strip_prefix('\u{feff}').unwrap_or(text);
        let value: Value =
            serde_json::from_str(text).map_err(|e| ManifestError::InvalidJson(e.to_string()))?;
        let Value::Object(map) = value else {
            return Err(ManifestError::NotAnObject);
        };
        let icons = match map.get("icons") {
            Some(Value::Array(items)) => items
                .iter()
                .filter_map(|v| v.as_object())
                .take(MAX_ICONS)
                .map(|o| ManifestIcon {
                    src: string(o, "src"),
                    sizes: string(o, "sizes"),
                    purpose: string(o, "purpose"),
                })
                .collect(),
            _ => Vec::new(),
        };
        Ok(Self {
            name: string(&map, "name"),
            short_name: string(&map, "short_name"),
            id: string(&map, "id"),
            start_url: string(&map, "start_url"),
            scope: string(&map, "scope"),
            display: string(&map, "display"),
            theme_color: string(&map, "theme_color"),
            background_color: string(&map, "background_color"),
            icons,
        })
    }
}

fn string(map: &Map<String, Value>, key: &str) -> Option<String> {
    match map.get(key) {
        Some(Value::String(s)) => Some(s.clone()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn garbage_json_yields_an_error_not_a_panic() {
        for input in [
            "",
            "not json",
            "{",
            "\u{0}",
            "{\"name\":}",
            "[]",
            "null",
            "\"s\"",
            "42",
        ] {
            assert!(Manifest::parse(input).is_err(), "input {input:?}");
        }
    }

    #[test]
    fn non_object_json_is_reported_as_not_an_object() {
        assert_eq!(Manifest::parse("[]"), Err(ManifestError::NotAnObject));
    }

    #[test]
    fn oversized_input_is_rejected_before_parsing() {
        let big = format!("{{\"name\":\"{}\"}}", "a".repeat(MAX_MANIFEST_BYTES));
        assert!(matches!(
            Manifest::parse(&big),
            Err(ManifestError::TooLarge { .. })
        ));
    }

    #[test]
    fn deeply_nested_json_is_an_error_not_a_stack_overflow() {
        let deep_array = "[".repeat(200_000);
        assert!(matches!(
            Manifest::parse(&deep_array),
            Err(ManifestError::InvalidJson(_))
        ));
        let nested_member = format!(
            "{{\"name\":{}{}}}",
            "[".repeat(100_000),
            "]".repeat(100_000)
        );
        assert!(matches!(
            Manifest::parse(&nested_member),
            Err(ManifestError::InvalidJson(_))
        ));
    }

    #[test]
    fn wrong_json_types_become_absent_for_every_field() {
        let m = Manifest::parse(
            r#"{"name":1,"short_name":[],"id":{},"start_url":true,"scope":null,
                "display":5,"theme_color":1.5,"background_color":[1],"icons":"x"}"#,
        )
        .unwrap();
        assert_eq!(m, Manifest::default());
    }

    #[test]
    fn wrong_typed_icon_entries_and_members_are_skipped() {
        let m = Manifest::parse(
            r#"{"icons":[1,"a",null,[],{"src":5,"sizes":[],"purpose":{}},{"src":"/a.png"}]}"#,
        )
        .unwrap();
        assert_eq!(m.icons.len(), 2);
        assert_eq!(m.icons[0], ManifestIcon::default());
        assert_eq!(m.icons[1].src.as_deref(), Some("/a.png"));
    }

    #[test]
    fn icon_list_is_capped() {
        let entries = vec![r#"{"src":"/a.png"}"#; 1000].join(",");
        let m = Manifest::parse(&format!("{{\"icons\":[{entries}]}}")).unwrap();
        assert_eq!(m.icons.len(), MAX_ICONS);
    }

    #[test]
    fn unknown_members_are_ignored_and_a_bom_is_tolerated() {
        let m = Manifest::parse("\u{feff}{\"name\":\"A\",\"shortcuts\":[1],\"x\":{}}").unwrap();
        assert_eq!(m.name.as_deref(), Some("A"));
    }
}
