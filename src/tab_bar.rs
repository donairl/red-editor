//! VS Code-style tab strip drawn above the editor.
//!
//! Knows nothing about documents: `app.rs` describes each tab as a
//! [`TabView`] and applies the returned [`TabBarAction`].

use eframe::egui::{self, Color32};

use crate::highlighter::Language;

/// What the strip needs to know about one open document.
#[derive(Clone, Debug, PartialEq)]
pub struct TabView {
    /// File name, or the placeholder for a new document. No dirty marker:
    /// the close slot shows that.
    pub title: String,
    /// Full path shown on hover; `None` for a document never saved.
    pub tooltip: Option<String>,
    pub language: Language,
    pub dirty: bool,
}

/// Colours of the strip, after VS Code's "Dark Modern" and "Light Modern".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TabPalette {
    pub strip: Color32,
    pub tab_inactive: Color32,
    /// Also the editor background, so the active tab joins the editor.
    pub tab_active: Color32,
    pub tab_hover: Color32,
    pub border: Color32,
    pub accent: Color32,
    pub fg_active: Color32,
    pub fg_inactive: Color32,
    /// Rounded box behind the `×` under the pointer.
    pub close_hover: Color32,
}

impl TabPalette {
    pub const DARK: TabPalette = TabPalette {
        strip: Color32::from_rgb(0x18, 0x18, 0x18),
        tab_inactive: Color32::from_rgb(0x18, 0x18, 0x18),
        tab_active: Color32::from_rgb(0x1F, 0x1F, 0x1F),
        tab_hover: Color32::from_rgb(0x1F, 0x1F, 0x1F),
        border: Color32::from_rgb(0x2B, 0x2B, 0x2B),
        accent: Color32::from_rgb(0x00, 0x78, 0xD4),
        fg_active: Color32::from_rgb(0xFF, 0xFF, 0xFF),
        fg_inactive: Color32::from_rgb(0x9D, 0x9D, 0x9D),
        close_hover: Color32::from_rgb(0x31, 0x31, 0x31),
    };

    pub const LIGHT: TabPalette = TabPalette {
        strip: Color32::from_rgb(0xF8, 0xF8, 0xF8),
        tab_inactive: Color32::from_rgb(0xF8, 0xF8, 0xF8),
        tab_active: Color32::from_rgb(0xFF, 0xFF, 0xFF),
        tab_hover: Color32::from_rgb(0xFF, 0xFF, 0xFF),
        border: Color32::from_rgb(0xE5, 0xE5, 0xE5),
        accent: Color32::from_rgb(0x00, 0x5F, 0xB8),
        fg_active: Color32::from_rgb(0x3B, 0x3B, 0x3B),
        fg_inactive: Color32::from_rgb(0x86, 0x86, 0x86),
        close_hover: Color32::from_rgb(0xE9, 0xE9, 0xE9),
    };

    /// The palette matching egui's current light or dark mode.
    pub fn for_visuals(visuals: &egui::Visuals) -> TabPalette {
        if visuals.dark_mode {
            Self::DARK
        } else {
            Self::LIGHT
        }
    }
}

/// Short coloured label shown before a tab's name. A `None` colour means
/// "use the inactive foreground".
pub fn badge(language: Language) -> (&'static str, Option<Color32>) {
    match language {
        Language::PlainText => ("TXT", None),
        Language::Rust => ("RS", Some(Color32::from_rgb(0xDE, 0xA5, 0x84))),
        Language::C => ("C", Some(Color32::from_rgb(0x51, 0x9A, 0xBA))),
        Language::Cpp => ("C++", Some(Color32::from_rgb(0xF3, 0x4B, 0x7D))),
        Language::Php => ("PHP", Some(Color32::from_rgb(0xA0, 0x74, 0xC4))),
        Language::JavaScript => ("JS", Some(Color32::from_rgb(0xCB, 0xCB, 0x41))),
        Language::TypeScript => ("TS", Some(Color32::from_rgb(0x51, 0x9A, 0xBA))),
        Language::Html => ("<>", Some(Color32::from_rgb(0xE4, 0x4D, 0x26))),
    }
}

/// What the close slot at the right of a tab shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CloseSlot {
    Empty,
    Dot,
    Cross,
    /// `×` on a rounded hover box.
    CrossHovered,
}

/// VS Code's rule for the close slot.
fn close_slot(slot_hovered: bool, dirty: bool, active: bool, tab_hovered: bool) -> CloseSlot {
    if slot_hovered {
        CloseSlot::CrossHovered
    } else if dirty {
        CloseSlot::Dot
    } else if active || tab_hovered {
        CloseSlot::Cross
    } else {
        CloseSlot::Empty
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dark_palette_matches_vscode_dark_modern() {
        let p = TabPalette::DARK;
        assert_eq!(p.strip, Color32::from_rgb(0x18, 0x18, 0x18));
        assert_eq!(p.tab_inactive, Color32::from_rgb(0x18, 0x18, 0x18));
        assert_eq!(p.tab_active, Color32::from_rgb(0x1F, 0x1F, 0x1F));
        assert_eq!(p.tab_hover, Color32::from_rgb(0x1F, 0x1F, 0x1F));
        assert_eq!(p.border, Color32::from_rgb(0x2B, 0x2B, 0x2B));
        assert_eq!(p.accent, Color32::from_rgb(0x00, 0x78, 0xD4));
        assert_eq!(p.fg_active, Color32::from_rgb(0xFF, 0xFF, 0xFF));
        assert_eq!(p.fg_inactive, Color32::from_rgb(0x9D, 0x9D, 0x9D));
    }

    #[test]
    fn test_light_palette_matches_vscode_light_modern() {
        let p = TabPalette::LIGHT;
        assert_eq!(p.strip, Color32::from_rgb(0xF8, 0xF8, 0xF8));
        assert_eq!(p.tab_inactive, Color32::from_rgb(0xF8, 0xF8, 0xF8));
        assert_eq!(p.tab_active, Color32::from_rgb(0xFF, 0xFF, 0xFF));
        assert_eq!(p.tab_hover, Color32::from_rgb(0xFF, 0xFF, 0xFF));
        assert_eq!(p.border, Color32::from_rgb(0xE5, 0xE5, 0xE5));
        assert_eq!(p.accent, Color32::from_rgb(0x00, 0x5F, 0xB8));
        assert_eq!(p.fg_active, Color32::from_rgb(0x3B, 0x3B, 0x3B));
        assert_eq!(p.fg_inactive, Color32::from_rgb(0x86, 0x86, 0x86));
    }

    #[test]
    fn test_palette_follows_egui_theme() {
        assert_eq!(TabPalette::for_visuals(&egui::Visuals::dark()), TabPalette::DARK);
        assert_eq!(TabPalette::for_visuals(&egui::Visuals::light()), TabPalette::LIGHT);
    }

    #[test]
    fn test_every_language_has_a_badge() {
        for lang in Language::all() {
            assert!(!badge(*lang).0.is_empty(), "no badge for {lang:?}");
        }
        assert_eq!(badge(Language::Rust), ("RS", Some(Color32::from_rgb(0xDE, 0xA5, 0x84))));
        assert_eq!(badge(Language::Html), ("<>", Some(Color32::from_rgb(0xE4, 0x4D, 0x26))));
        assert_eq!(badge(Language::PlainText), ("TXT", None));
    }

    /// The slot under the pointer always offers `×`; otherwise unsaved
    /// changes win over "active or hovered", so an active dirty tab keeps
    /// its dot.
    #[test]
    fn test_close_slot_rule() {
        use CloseSlot::*;
        // (slot hovered, dirty, active, tab hovered) -> slot
        assert_eq!(close_slot(true, true, false, true), CrossHovered);
        assert_eq!(close_slot(true, false, false, true), CrossHovered);
        assert_eq!(close_slot(false, true, true, false), Dot);
        assert_eq!(close_slot(false, true, false, true), Dot);
        assert_eq!(close_slot(false, false, true, false), Cross);
        assert_eq!(close_slot(false, false, false, true), Cross);
        assert_eq!(close_slot(false, false, false, false), Empty);
    }
}
