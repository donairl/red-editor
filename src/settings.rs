//! The editor settings the Settings window edits: indentation and what an
//! explicit save does to the text. Kept in eframe's storage as one JSON
//! object; anything unreadable falls back to the defaults, field by field.

use serde_json::{Value, json};

/// Fewest and most spaces one indent level may be.
pub const MIN_TAB_SIZE: u8 = 1;
pub const MAX_TAB_SIZE: u8 = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EditorSettings {
    /// Spaces per indent level when `insert_spaces` is on. egui always draws
    /// a `\t` four columns wide, so this is not the width of a tab.
    pub tab_size: u8,
    /// Indent with spaces, or with one `\t` per level.
    pub insert_spaces: bool,
    /// Strip spaces and tabs at line ends on an explicit save.
    pub trim_trailing_whitespace: bool,
    /// Run the document's formatter on an explicit save.
    pub format_on_save: bool,
}

impl Default for EditorSettings {
    fn default() -> Self {
        Self {
            tab_size: 4,
            insert_spaces: true,
            trim_trailing_whitespace: false,
            format_on_save: false,
        }
    }
}

impl EditorSettings {
    /// One indent level: `"\t"`, or `tab_size` spaces.
    pub fn indent_unit(&self) -> String {
        if self.insert_spaces {
            " ".repeat(usize::from(self.tab_size))
        } else {
            "\t".to_owned()
        }
    }

    /// The settings as eframe's storage keeps them.
    pub fn to_storage(self) -> String {
        json!({
            "tab_size": self.tab_size,
            "insert_spaces": self.insert_spaces,
            "trim_trailing_whitespace": self.trim_trailing_whitespace,
            "format_on_save": self.format_on_save,
        })
        .to_string()
    }

    /// Reads what [`Self::to_storage`] wrote. A missing or unreadable text
    /// gives the defaults; so does each field that is missing or of the
    /// wrong type. `tab_size` is clamped into range.
    pub fn from_storage(text: Option<&str>) -> Self {
        let defaults = Self::default();
        let Some(Value::Object(map)) = text.and_then(|t| serde_json::from_str(t).ok()) else {
            return defaults;
        };
        let flag = |key: &str, default: bool| map.get(key).and_then(Value::as_bool).unwrap_or(default);
        let tab_size = map
            .get("tab_size")
            .and_then(Value::as_i64)
            .map(|n| n.clamp(i64::from(MIN_TAB_SIZE), i64::from(MAX_TAB_SIZE)) as u8)
            .unwrap_or(defaults.tab_size);
        Self {
            tab_size,
            insert_spaces: flag("insert_spaces", defaults.insert_spaces),
            trim_trailing_whitespace: flag("trim_trailing_whitespace", defaults.trim_trailing_whitespace),
            format_on_save: flag("format_on_save", defaults.format_on_save),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_defaults() {
        let s = EditorSettings::default();
        assert_eq!(s.tab_size, 4);
        assert!(s.insert_spaces);
        assert!(!s.trim_trailing_whitespace);
        assert!(!s.format_on_save);
    }

    #[test]
    fn test_storage_round_trip() {
        let s = EditorSettings {
            tab_size: 2,
            insert_spaces: false,
            trim_trailing_whitespace: true,
            format_on_save: true,
        };
        assert_eq!(EditorSettings::from_storage(Some(&s.to_storage())), s);
    }

    #[test]
    fn test_unreadable_storage_gives_defaults() {
        for text in [None, Some(""), Some("junk"), Some("[1]"), Some("3")] {
            assert_eq!(EditorSettings::from_storage(text), EditorSettings::default(), "{text:?}");
        }
    }

    #[test]
    fn test_each_bad_field_takes_its_default() {
        let s = EditorSettings::from_storage(Some(
            r#"{"tab_size": "x", "insert_spaces": false, "format_on_save": 1}"#,
        ));
        assert_eq!(
            s,
            EditorSettings { insert_spaces: false, ..EditorSettings::default() }
        );
    }

    #[test]
    fn test_tab_size_is_clamped() {
        assert_eq!(EditorSettings::from_storage(Some(r#"{"tab_size": 0}"#)).tab_size, 1);
        assert_eq!(EditorSettings::from_storage(Some(r#"{"tab_size": 99}"#)).tab_size, 8);
        assert_eq!(EditorSettings::from_storage(Some(r#"{"tab_size": -3}"#)).tab_size, 1);
    }

    #[test]
    fn test_indent_unit() {
        let spaces = EditorSettings { tab_size: 2, ..EditorSettings::default() };
        assert_eq!(spaces.indent_unit(), "  ");
        let tabs = EditorSettings { insert_spaces: false, ..EditorSettings::default() };
        assert_eq!(tabs.indent_unit(), "\t");
    }
}
