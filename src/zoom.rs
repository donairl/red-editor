//! The editor's font size: the one knob that makes the text bigger or
//! smaller, everywhere it is shown.
//!
//! The size is a plain number of points, kept in the app and written into
//! egui's monospace text style. Everything that draws code — the editor, the
//! gutter, the diff view, the terminal — asks egui for the monospace font,
//! so one number moves all of them.

use eframe::egui::{self, FontFamily, FontId};

/// The font size the editor starts with, and the one `Ctrl+0` returns to.
///
/// The same size egui itself uses for monospace text, so nothing looks
/// different until the user zooms.
pub const DEFAULT_SIZE: f32 = 13.0;

/// The smallest font size: below this the text stops being readable.
pub const MIN_SIZE: f32 = 8.0;

/// The largest font size: above this the text no longer fits a line.
pub const MAX_SIZE: f32 = 48.0;

/// How much one step of the zoom changes the size.
pub const STEP: i32 = 1;

/// How much the font grows for one notch of the mouse wheel.
pub const SCROLL_FACTOR: f32 = 1.1;

/// The points one notch of the mouse wheel is worth.
///
/// egui counts a line of scrolling as this many points, so a wheel notch
/// and a trackpad flick that travel the same distance zoom the same way.
pub const NOTCH_POINTS: f32 = 40.0;

/// The font size, clamped to the range the editor allows.
///
/// A size that is not a number (a corrupt setting, say) falls back to
/// [`DEFAULT_SIZE`], so the editor always has a usable font.
pub fn clamp(size: f32) -> f32 {
    if !size.is_finite() {
        return DEFAULT_SIZE;
    }
    size.clamp(MIN_SIZE, MAX_SIZE)
}

/// The size `steps` steps away from `size`: positive makes it bigger.
///
/// The result is a whole number of points: a size a scroll left at a
/// fraction moves to the next whole point in the direction of travel, so the
/// keyboard steps stay even.
pub fn by(size: f32, steps: i32) -> f32 {
    match steps {
        i32::MIN..=-1 => clamp(size.ceil() + steps as f32 * STEP as f32),
        1..=i32::MAX => clamp(size.floor() + steps as f32 * STEP as f32),
        _ => clamp(size),
    }
}

/// The size `Ctrl+0` returns to.
pub fn reset() -> f32 {
    DEFAULT_SIZE
}

/// The size as a percentage of the default, as the status bar shows it.
pub fn percent(size: f32) -> i32 {
    ((size / DEFAULT_SIZE) * 100.0).round() as i32
}

/// The points a wheel event of `delta` in `unit` is worth.
///
/// A wheel notch arrives as a line, a trackpad as points; both are turned
/// into points so one notch zooms the same wherever it comes from.
pub fn wheel_points(unit: egui::MouseWheelUnit, delta: f32) -> f32 {
    match unit {
        egui::MouseWheelUnit::Line => delta * NOTCH_POINTS,
        egui::MouseWheelUnit::Point => delta,
        egui::MouseWheelUnit::Page => delta * NOTCH_POINTS * 10.0,
    }
}

/// The size after a `Ctrl` + scroll of `points`.
///
/// Scrolling up (a positive delta) makes the text bigger. The size grows by
/// a factor rather than by a step, so a slow trackpad scroll and a quick
/// wheel notch both feel the same: the first few points of a scroll move
/// the size by less than a point, and that fraction is kept instead of
/// being rounded away.
pub fn scrolled(size: f32, points: f32) -> f32 {
    if !points.is_finite() || points == 0.0 {
        return clamp(size);
    }
    let factor = (SCROLL_FACTOR.ln() * points / NOTCH_POINTS).exp();
    clamp(size * factor)
}

/// The size read from storage; [`DEFAULT_SIZE`] when there is none.
pub fn from_storage(text: Option<&str>) -> f32 {
    text.and_then(|text| text.trim().parse::<f32>().ok())
        .map(clamp)
        .unwrap_or(DEFAULT_SIZE)
}

/// The size as it is written to storage.
///
/// One decimal is enough to keep the fraction a scroll left, and short
/// enough to read.
pub fn to_storage(size: f32) -> String {
    format!("{:.1}", clamp(size))
}

/// Writes the size into the monospace text style of `style`.
///
/// Everything that draws code asks egui for the monospace font, so this one
/// line is what makes the whole editor bigger or smaller.
pub fn apply_to_style(style: &mut egui::Style, size: f32) {
    let size = clamp(size);
    let mut font = style
        .text_styles
        .get(&egui::TextStyle::Monospace)
        .cloned()
        .unwrap_or_else(|| FontId::new(size, FontFamily::Monospace));
    font.size = size;
    style.text_styles.insert(egui::TextStyle::Monospace, font);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_the_default_size_is_inside_the_range() {
        const { assert!(DEFAULT_SIZE >= MIN_SIZE && DEFAULT_SIZE <= MAX_SIZE) };
        assert_eq!(clamp(DEFAULT_SIZE), DEFAULT_SIZE);
    }

    #[test]
    fn test_clamp_holds_the_size_inside_the_range() {
        assert_eq!(clamp(MIN_SIZE - 10.0), MIN_SIZE);
        assert_eq!(clamp(MAX_SIZE + 10.0), MAX_SIZE);
        assert_eq!(clamp(20.0), 20.0);
    }

    #[test]
    fn test_a_size_that_is_not_a_number_falls_back_to_the_default() {
        assert_eq!(clamp(f32::NAN), DEFAULT_SIZE);
        assert_eq!(clamp(f32::INFINITY), DEFAULT_SIZE);
        assert_eq!(clamp(f32::NEG_INFINITY), DEFAULT_SIZE);
    }

    #[test]
    fn test_stepping_up_and_down_moves_by_one_point() {
        assert_eq!(by(DEFAULT_SIZE, STEP), DEFAULT_SIZE + STEP as f32);
        assert_eq!(by(DEFAULT_SIZE, -STEP), DEFAULT_SIZE - STEP as f32);
        assert_eq!(by(DEFAULT_SIZE, 0), DEFAULT_SIZE);
    }

    #[test]
    fn test_stepping_stops_at_the_edges() {
        assert_eq!(by(MAX_SIZE, 1), MAX_SIZE);
        assert_eq!(by(MIN_SIZE, -1), MIN_SIZE);
        assert_eq!(by(MAX_SIZE, 10), MAX_SIZE);
        assert_eq!(by(MIN_SIZE, -10), MIN_SIZE);
    }

    /// A size a scroll left at a fraction moves to a whole point in the
    /// direction of travel.
    #[test]
    fn test_stepping_moves_to_a_whole_point() {
        assert_eq!(by(14.4, 1), 15.0);
        assert_eq!(by(14.4, -1), 14.0);
        assert_eq!(by(14.0, 1), 15.0);
        assert_eq!(by(14.0, -1), 13.0);
    }

    #[test]
    fn test_reset_returns_to_the_default() {
        assert_eq!(reset(), DEFAULT_SIZE);
    }

    #[test]
    fn test_the_percentage_follows_the_default() {
        assert_eq!(percent(DEFAULT_SIZE), 100);
        assert_eq!(percent(DEFAULT_SIZE * 2.0), 200);
        assert_eq!(percent(DEFAULT_SIZE / 2.0), 50);
    }

    #[test]
    fn test_a_wheel_notch_grows_the_font_by_about_a_tenth() {
        let bigger = scrolled(DEFAULT_SIZE, NOTCH_POINTS);
        assert!(bigger > DEFAULT_SIZE, "{bigger} is not bigger");
        assert!(
            (bigger - DEFAULT_SIZE * SCROLL_FACTOR).abs() < 0.01,
            "one notch: {bigger}"
        );
    }

    #[test]
    fn test_scrolling_up_makes_the_text_bigger_and_down_smaller() {
        assert!(scrolled(DEFAULT_SIZE, NOTCH_POINTS) > DEFAULT_SIZE);
        assert!(scrolled(DEFAULT_SIZE, -NOTCH_POINTS) < DEFAULT_SIZE);
    }

    #[test]
    fn test_scrolling_stops_at_the_edges() {
        assert_eq!(scrolled(MAX_SIZE, 1000.0), MAX_SIZE);
        assert_eq!(scrolled(MIN_SIZE, -1000.0), MIN_SIZE);
    }

    /// A slow trackpad scroll moves the size by less than a point at a time.
    /// The fraction is kept, so the size still moves.
    #[test]
    fn test_a_slow_scroll_still_moves_the_size() {
        let once = scrolled(DEFAULT_SIZE, 1.0);
        assert!(once > DEFAULT_SIZE, "{once} did not move");
        assert!(once < DEFAULT_SIZE + 1.0, "{once} moved too far");
    }

    #[test]
    fn test_a_scroll_too_small_to_move_a_step_changes_nothing() {
        assert_eq!(scrolled(DEFAULT_SIZE, 0.0), DEFAULT_SIZE);
        assert_eq!(scrolled(DEFAULT_SIZE, f32::NAN), DEFAULT_SIZE);
    }

    #[test]
    fn test_a_wheel_notch_is_worth_the_same_in_lines_and_in_points() {
        let lines = wheel_points(egui::MouseWheelUnit::Line, 1.0);
        let points = wheel_points(egui::MouseWheelUnit::Point, NOTCH_POINTS);
        assert_eq!(lines, points);
    }

    #[test]
    fn test_storage_round_trip() {
        assert_eq!(from_storage(Some(&to_storage(20.0))), 20.0);
        assert_eq!(from_storage(None), DEFAULT_SIZE);
        assert_eq!(from_storage(Some("")), DEFAULT_SIZE);
        assert_eq!(from_storage(Some("not a number")), DEFAULT_SIZE);
    }

    /// A size that is out of range in the setting is brought back in.
    #[test]
    fn test_storage_clamps_the_size() {
        assert_eq!(from_storage(Some("1000")), MAX_SIZE);
        assert_eq!(from_storage(Some("1")), MIN_SIZE);
    }

    /// The size keeps the fraction a scroll left, to one decimal.
    #[test]
    fn test_storage_keeps_one_decimal() {
        assert_eq!(to_storage(14.34), "14.3");
        assert_eq!(from_storage(Some("14.3")), 14.3);
    }

    /// The monospace text style of the style carries the size, so every part
    /// of the editor that draws code grows with it.
    #[test]
    fn test_the_size_reaches_the_monospace_text_style() {
        let mut style = egui::Style::default();
        apply_to_style(&mut style, 20.0);
        let font = style.text_styles[&egui::TextStyle::Monospace].clone();
        assert_eq!(font.size, 20.0);
        assert_eq!(font.family, FontFamily::Monospace);
    }

    /// A style without a monospace text style gets one.
    #[test]
    fn test_a_style_without_a_monospace_font_gets_one() {
        let mut style = egui::Style::default();
        style.text_styles.remove(&egui::TextStyle::Monospace);
        apply_to_style(&mut style, 20.0);
        assert_eq!(style.text_styles[&egui::TextStyle::Monospace].size, 20.0);
    }
}
