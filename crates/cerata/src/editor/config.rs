//! Editor settings and the vertical metrics of Writer's theme
//! (`prosemark-theme.css:155-187` and `:259-271`).

use cerata_core::settings::ConfigValue;
use cerata_markdown::LineStyle;
use gpui::SharedString;

/// Reference body size of Writer's spacing scale (`--ws-px`).
const DESIGN_PX: f32 = 16.0;
/// Font ascent plus descent, in em (`--ws-glyph`).
const GLYPH: f32 = 1.125;
/// Bold display sizes run taller (`--ws-heading-glyph`).
const HEADING_GLYPH: f32 = 1.17;
/// Line height of heading lines, relative to their own size.
const HEADING_LINE_HEIGHT: f32 = 1.3;
/// Text column bounds (`editor.content-width` range in the settings schema).
pub const MIN_CONTENT_WIDTH: f32 = 480.0;
pub const MAX_CONTENT_WIDTH: f32 = 1600.0;

#[derive(Clone, Debug, PartialEq)]
pub struct EditorConfig {
    /// `editor.font-size`, in px.
    pub font_size: f32,
    /// `editor.zoom`, as a factor (100 % is 1.0).
    pub zoom: f32,
    /// `editor.line-height`, a multiplier of the font size.
    pub line_height: f32,
    /// `editor.content-width`, in px, clamped to its range.
    pub content_width: f32,
    /// `fonts.editor` and `fonts.mono`, CSS family lists.
    pub font_families: String,
    pub mono_families: String,
}

impl Default for EditorConfig {
    fn default() -> Self {
        Self {
            font_size: 16.0,
            zoom: 1.0,
            line_height: 1.875,
            content_width: 734.0,
            font_families: String::new(),
            mono_families: String::new(),
        }
    }
}

impl EditorConfig {
    /// From merged settings; missing or invalid values keep the defaults.
    pub fn from_settings(settings: &[(&str, ConfigValue)]) -> Self {
        let mut config = Self::default();
        let number = |key: &str| {
            settings
                .iter()
                .find(|(k, _)| *k == key)
                .and_then(|(_, v)| v.as_f64())
                .filter(|v| v.is_finite() && *v > 0.0)
        };
        let text = |key: &str| {
            settings
                .iter()
                .find(|(k, _)| *k == key)
                .and_then(|(_, v)| v.as_str().map(str::to_owned))
        };
        if let Some(size) = number("editor.font-size") {
            config.font_size = size as f32;
        }
        if let Some(zoom) = number("editor.zoom") {
            config.zoom = (zoom as f32 / 100.0).clamp(0.5, 3.0);
        }
        if let Some(line_height) = number("editor.line-height") {
            config.line_height = line_height as f32;
        }
        if let Some(width) = number("editor.content-width") {
            config.content_width = (width as f32).clamp(MIN_CONTENT_WIDTH, MAX_CONTENT_WIDTH);
        }
        config.font_families = text("fonts.editor").unwrap_or_default();
        config.mono_families = text("fonts.mono").unwrap_or_default();
        config
    }

    /// Body text size: the font size times the zoom.
    pub fn text_size(&self) -> f32 {
        self.font_size * self.zoom
    }

    /// Height of one body row.
    pub fn row_height(&self) -> f32 {
        self.line_height * self.text_size()
    }

    /// Vertical metrics of a line with `style` (`None`: body text).
    pub fn metrics(&self, style: Option<LineStyle>) -> LineMetrics {
        let base = self.text_size();
        match style {
            Some(LineStyle::Heading(level)) => {
                let scale = LineStyle::Heading(level).scale();
                let px = base / DESIGN_PX;
                let line_gap = (self.line_height - GLYPH) * base;
                let leading = (HEADING_LINE_HEIGHT - HEADING_GLYPH) * scale * base / 2.0;
                LineMetrics {
                    font_size: base * scale,
                    row_height: HEADING_LINE_HEIGHT * base * scale,
                    pad_top: (12.0 * px - leading).max(0.0),
                    pad_bottom: (8.0 * px - line_gap / 2.0 - leading).max(0.0),
                }
            }
            _ => LineMetrics {
                font_size: base,
                row_height: self.row_height(),
                pad_top: 0.0,
                pad_bottom: 0.0,
            },
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LineMetrics {
    pub font_size: f32,
    pub row_height: f32,
    pub pad_top: f32,
    pub pad_bottom: f32,
}

/// CSS generic families and keywords that name no installed font.
const GENERIC: &[&str] = &[
    "serif",
    "sans-serif",
    "monospace",
    "system-ui",
    "ui-sans-serif",
    "ui-monospace",
    "ui-serif",
    "cursive",
    "fantasy",
];

/// The first family of the CSS list `families` that is installed, else the
/// platform's default (`mono` for code).
pub fn resolve_family(families: &str, installed: &[String], mono: bool) -> SharedString {
    for family in families.split(',') {
        let family = family.trim().trim_matches(['"', '\'']).trim();
        if family.is_empty() || family.starts_with('-') || GENERIC.contains(&family) {
            continue;
        }
        if installed
            .iter()
            .any(|name| name.eq_ignore_ascii_case(family))
        {
            return SharedString::from(family.to_owned());
        }
    }
    SharedString::new_static(default_family(mono))
}

fn default_family(mono: bool) -> &'static str {
    match (cfg!(target_os = "macos"), cfg!(target_os = "windows"), mono) {
        (true, _, false) => ".SystemUIFont",
        (true, _, true) => "Menlo",
        (_, true, false) => "Segoe UI",
        (_, true, true) => "Consolas",
        (_, _, false) => "Noto Sans",
        (_, _, true) => "DejaVu Sans Mono",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_drive_size_zoom_line_height_and_width() {
        let config = EditorConfig::from_settings(&[
            ("editor.font-size", ConfigValue::Number(18.0)),
            ("editor.zoom", ConfigValue::Number(150.0)),
            ("editor.line-height", ConfigValue::Number(1.5)),
            ("editor.content-width", ConfigValue::Number(3000.0)),
        ]);
        assert_eq!(config.text_size(), 27.0);
        assert_eq!(config.row_height(), 40.5);
        assert_eq!(config.content_width, MAX_CONTENT_WIDTH);
        let defaults = EditorConfig::from_settings(&[]);
        assert_eq!(defaults.content_width, 734.0);
        assert_eq!(defaults.row_height(), 30.0);
    }

    #[test]
    fn heading_metrics_follow_writers_spacing_scale() {
        let config = EditorConfig::default();
        let h1 = config.metrics(Some(LineStyle::Heading(1)));
        assert_eq!(h1.font_size, 25.6);
        assert!((h1.row_height - 33.28).abs() < 1e-4);
        // leading = 0.13 * 1.6 * 16 / 2 = 1.664
        assert!((h1.pad_top - (12.0 - 1.664)).abs() < 1e-4);
        // 8 - (1.875 - 1.125) * 16 / 2 - 1.664
        assert!((h1.pad_bottom - 0.336).abs() < 1e-4);
        let body = config.metrics(None);
        assert_eq!((body.font_size, body.row_height), (16.0, 30.0));
    }

    #[test]
    fn families_resolve_to_the_first_installed_one() {
        let installed = vec!["Segoe UI".to_owned(), "Consolas".to_owned()];
        let list = "\"SF Pro\", -apple-system-body, ui-sans-serif, system-ui, \"Segoe UI\", Arial";
        assert_eq!(resolve_family(list, &installed, false).as_ref(), "Segoe UI");
        assert_eq!(
            resolve_family("\"SF Mono\", Consolas, monospace", &installed, true).as_ref(),
            "Consolas"
        );
    }
}
