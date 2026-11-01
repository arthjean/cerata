//! Themes (`lib/theme.ts`, `App.css:52-94`): five values per mode (accent,
//! background, foreground, translucency, contrast) from which every color
//! token derives; the preference is system, light or dark, and system
//! follows the OS. The four presets of Writer's `shared/themes` are loaded
//! and selectable (Writer never offered them); Writer's own preset is
//! renamed Cerata.

use cerata_core::settings::Settings;
use gpui::{Hsla, Rgba, WindowAppearance};
use serde::Deserialize;

pub const THEME_KEY: &str = "appearance.theme";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Preference {
    #[default]
    System,
    Light,
    Dark,
}

impl Preference {
    /// An invalid value is `system`.
    pub fn parse(id: Option<&str>) -> Self {
        match id {
            Some("light") => Self::Light,
            Some("dark") => Self::Dark,
            _ => Self::System,
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Light => "light",
            Self::Dark => "dark",
        }
    }

    /// "Toggle Dark Mode": system, light, dark, system (`use-theme.ts:7`).
    pub fn next(self) -> Self {
        match self {
            Self::System => Self::Light,
            Self::Light => Self::Dark,
            Self::Dark => Self::System,
        }
    }

    pub fn mode(self, appearance: WindowAppearance) -> Mode {
        match self {
            Self::Light => Mode::Light,
            Self::Dark => Mode::Dark,
            Self::System => match appearance {
                WindowAppearance::Dark | WindowAppearance::VibrantDark => Mode::Dark,
                WindowAppearance::Light | WindowAppearance::VibrantLight => Mode::Light,
            },
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Light,
    Dark,
}

impl Mode {
    pub fn id(self) -> &'static str {
        match self {
            Self::Light => "light",
            Self::Dark => "dark",
        }
    }
}

/// The five values of one mode, as stored in `theme.{mode}.*` and in the
/// preset files.
#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct ModeValues {
    pub accent: String,
    pub background: String,
    pub foreground: String,
    pub translucent: f64,
    pub contrast: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Preset {
    pub name: String,
    pub light: ModeValues,
    pub dark: ModeValues,
}

const BUILTIN: &[(&str, &str, &str)] = &[
    (
        "Cerata",
        include_str!("../assets/themes/cerata/light.json"),
        include_str!("../assets/themes/cerata/dark.json"),
    ),
    (
        "Default",
        include_str!("../assets/themes/default/light.json"),
        include_str!("../assets/themes/default/dark.json"),
    ),
    (
        "High Contrast",
        include_str!("../assets/themes/high-contrast/light.json"),
        include_str!("../assets/themes/high-contrast/dark.json"),
    ),
    (
        "Warm Paper",
        include_str!("../assets/themes/warm-paper/light.json"),
        include_str!("../assets/themes/warm-paper/dark.json"),
    ),
];

/// Presets from `(name, light JSON, dark JSON)`; an invalid one is skipped
/// with a warning.
pub fn load_presets(sources: &[(&str, &str, &str)]) -> Vec<Preset> {
    sources
        .iter()
        .filter_map(|(name, light, dark)| {
            let parse = |text: &str| -> Result<ModeValues, String> {
                let values: ModeValues = serde_json::from_str(text).map_err(|e| e.to_string())?;
                for color in [&values.accent, &values.background, &values.foreground] {
                    parse_hex(color).ok_or_else(|| format!("invalid color {color}"))?;
                }
                Ok(values)
            };
            match (parse(light), parse(dark)) {
                (Ok(light), Ok(dark)) => Some(Preset {
                    name: (*name).to_owned(),
                    light,
                    dark,
                }),
                (Err(error), _) | (_, Err(error)) => {
                    log::warn!("theme preset {name} ignored: {error}");
                    None
                }
            }
        })
        .collect()
}

pub fn presets() -> Vec<Preset> {
    load_presets(BUILTIN)
}

/// `#RGB` or `#RRGGBB`.
pub fn parse_hex(text: &str) -> Option<Rgba> {
    let hex = text.strip_prefix('#')?;
    let full = match hex.len() {
        3 => hex.chars().flat_map(|c| [c, c]).collect::<String>(),
        6 => hex.to_owned(),
        _ => return None,
    };
    let value = u32::from_str_radix(&full, 16).ok()?;
    Some(gpui::rgb(value))
}

/// `1 - t/100 × 0.95`, `t` clamped to 0..=100.
pub fn opacity(translucent: f64) -> f32 {
    (1.0 - translucent.clamp(0.0, 100.0) / 100.0 * 0.95) as f32
}

/// `0.2 + c/100 × 0.8`, `c` clamped to 0..=100.
pub fn contrast(contrast: f64) -> f32 {
    (0.2 + contrast.clamp(0.0, 100.0) / 100.0 * 0.8) as f32
}

/// The color tokens of one mode.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tokens {
    pub bg: Hsla,
    pub bg_base: Hsla,
    pub text: Hsla,
    pub text_secondary: Hsla,
    pub muted: Hsla,
    pub icon_muted: Hsla,
    pub border: Hsla,
    pub line_subtler: Hsla,
    pub focus_border: Hsla,
    pub surface_card: Hsla,
    pub surface_subtle: Hsla,
    pub surface_subtle_strong: Hsla,
    pub surface_input: Hsla,
    pub surface_selected: Hsla,
    pub surface_palette: Hsla,
    pub scrollbar_thumb: Hsla,
    pub accent: Hsla,
    pub selection: Hsla,
    pub dark: bool,
}

fn mix(color: Hsla, percent: f32) -> Hsla {
    color.opacity((percent / 100.0).clamp(0.0, 1.0))
}

/// The derived tokens (`App.css:52-94`): `mix(X, p)` is X at p % opacity.
pub fn tokens(values: &ModeValues, dark: bool) -> Tokens {
    let fallback = |text: &str, default: u32| -> Hsla {
        parse_hex(text).unwrap_or_else(|| gpui::rgb(default)).into()
    };
    let accent = fallback(&values.accent, 0xff6a00);
    let bg_base = fallback(&values.background, if dark { 0x111111 } else { 0xffffff });
    let fg = fallback(&values.foreground, if dark { 0xfcfcfc } else { 0x0d0d0d });
    let c = contrast(values.contrast);
    let o = opacity(values.translucent);
    Tokens {
        bg: mix(bg_base, o * 100.0),
        bg_base,
        text: fg,
        text_secondary: mix(fg, 80.0),
        // Writer's 54 % fails 4.5:1 on the sidebar in both modes (3.96 and
        // 4.49 measured); 68 % passes on every panel.
        muted: mix(fg, 68.0),
        icon_muted: mix(fg, 40.0),
        border: mix(fg, c * 24.0),
        line_subtler: mix(fg, c * 15.0),
        focus_border: mix(fg, c * 65.0),
        surface_card: mix(fg, c * 16.0),
        surface_subtle: mix(fg, c * 18.0),
        surface_subtle_strong: mix(fg, c * 36.0),
        surface_input: mix(fg, c * 28.0),
        surface_selected: mix(fg, c * 26.0),
        surface_palette: mix(bg_base, 80.0),
        scrollbar_thumb: mix(fg, c * 58.0),
        accent,
        selection: mix(accent, 30.0),
        dark,
    }
}

/// The five values of `mode` from the settings.
pub fn mode_values(settings: &Settings, mode: Mode) -> ModeValues {
    let key = |name: &str| format!("theme.{}.{name}", mode.id());
    let text = |name: &str| {
        settings
            .get(&key(name))
            .and_then(|v| v.as_str().map(str::to_owned))
            .unwrap_or_default()
    };
    let number = |name: &str| {
        settings
            .get(&key(name))
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0)
    };
    ModeValues {
        accent: text("accent"),
        background: text("background"),
        foreground: text("foreground"),
        translucent: number("translucent"),
        contrast: number("contrast"),
    }
}

/// The editor and chrome palette of `tokens`.
pub fn palette(tokens: &Tokens) -> crate::editor::theme::Palette {
    crate::editor::theme::Palette {
        background: tokens.bg,
        text: tokens.text,
        muted: tokens.muted,
        accent: tokens.accent,
        selection: tokens.selection,
        code_background: tokens.surface_card,
        quote_bar: tokens.border,
        rule: tokens.line_subtler,
        scrollbar: tokens.scrollbar_thumb,
        panel: tokens.surface_subtle,
        border: tokens.border,
        overlay: tokens.bg_base,
        danger: gpui::rgb(if tokens.dark { 0xffb3ab } else { 0xa40e26 }).into(),
        dark: tokens.dark,
    }
}

/// Applies the theme of `settings` for the OS `appearance`: the palette is
/// installed for every editor and chrome color; returns the tokens and
/// whether the window is translucent.
pub fn apply(settings: &Settings, appearance: WindowAppearance) -> (Tokens, bool) {
    let mode = preference(settings).mode(appearance);
    let values = mode_values(settings, mode);
    let tokens = tokens(&values, mode == Mode::Dark);
    crate::editor::theme::set(palette(&tokens));
    (tokens, values.translucent > 0.0)
}

pub fn preference(settings: &Settings) -> Preference {
    Preference::parse(
        settings
            .get(THEME_KEY)
            .and_then(|v| v.as_str().map(str::to_owned))
            .as_deref(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn values(t: f64, c: f64) -> ModeValues {
        ModeValues {
            accent: "#FF6A00".into(),
            background: "#FFFFFF".into(),
            foreground: "#1A1A1A".into(),
            translucent: t,
            contrast: c,
        }
    }

    #[test]
    fn the_formulas_match_writer() {
        assert_eq!(opacity(0.0), 1.0);
        assert!((opacity(100.0) - 0.05).abs() < 1e-6);
        assert!((opacity(20.0) - 0.81).abs() < 1e-6);
        assert!((contrast(0.0) - 0.2).abs() < 1e-6);
        assert!((contrast(100.0) - 1.0).abs() < 1e-6);
        assert_eq!(opacity(-5.0), 1.0, "clamped");
        let t = tokens(&values(20.0, 38.0), false);
        let c = contrast(38.0);
        assert!((t.border.a - c * 0.24).abs() < 1e-5);
        assert!((t.surface_selected.a - c * 0.26).abs() < 1e-5);
        assert!((t.bg.a - 0.81).abs() < 1e-5);
        assert!(
            (t.muted.a - 0.68).abs() < 1e-5,
            "raised from 54 % for contrast"
        );
        assert!((t.selection.a - 0.30).abs() < 1e-5);
    }

    fn over(top: Hsla, under: Rgba) -> Rgba {
        let top = Rgba::from(top);
        let a = top.a;
        Rgba {
            r: top.r * a + under.r * (1.0 - a),
            g: top.g * a + under.g * (1.0 - a),
            b: top.b * a + under.b * (1.0 - a),
            a: 1.0,
        }
    }

    fn ratio(a: Rgba, b: Rgba) -> f32 {
        let lum = |c: Rgba| {
            let ch = |v: f32| {
                if v <= 0.039_28 {
                    v / 12.92
                } else {
                    ((v + 0.055) / 1.055).powf(2.4)
                }
            };
            0.2126 * ch(c.r) + 0.7152 * ch(c.g) + 0.0722 * ch(c.b)
        };
        let (x, y) = (lum(a), lum(b));
        (x.max(y) + 0.05) / (x.min(y) + 0.05)
    }

    /// Muted and error text reach 4.5:1 on the window background and on a
    /// selected sidebar row (the darkest panel in light mode, the lightest
    /// in dark mode), for every preset.
    #[test]
    fn muted_and_danger_text_reach_4_5_to_1_on_every_panel() {
        for preset in presets() {
            for (values, dark) in [(&preset.light, false), (&preset.dark, true)] {
                let t = tokens(values, dark);
                let p = palette(&t);
                let base = Rgba::from(t.bg_base);
                let selected = over(t.border, base);
                for (name, fg) in [("muted", p.muted), ("danger", p.danger)] {
                    for (surface, bg) in [("window", base), ("selected row", selected)] {
                        let fg = over(fg, bg);
                        let r = ratio(fg, bg);
                        assert!(
                            r >= 4.5,
                            "{} {}: {name} on {surface} is {r:.2}:1",
                            preset.name,
                            if dark { "dark" } else { "light" }
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn the_four_presets_load_with_writers_values() {
        let presets = presets();
        let names: Vec<&str> = presets.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["Cerata", "Default", "High Contrast", "Warm Paper"]);
        let high = &presets[2];
        assert_eq!(high.light.accent, "#0040A0");
        assert_eq!(high.dark.contrast, 88.0);
        assert_eq!(presets[3].dark.background, "#1F1A14");
    }

    #[test]
    fn an_invalid_preset_is_ignored() {
        let good = include_str!("../assets/themes/default/light.json");
        let presets = load_presets(&[
            ("Broken", "{not json", good),
            (
                "Bad color",
                r##"{"accent":"red","background":"#fff","foreground":"#000","translucent":0,"contrast":0}"##,
                good,
            ),
            ("Default", good, good),
        ]);
        assert_eq!(presets.len(), 1);
        assert_eq!(presets[0].name, "Default");
    }

    #[test]
    fn the_preference_cycles_and_follows_the_os() {
        assert_eq!(Preference::parse(Some("bogus")), Preference::System);
        assert_eq!(Preference::System.next(), Preference::Light);
        assert_eq!(Preference::Light.next(), Preference::Dark);
        assert_eq!(Preference::Dark.next(), Preference::System);
        assert_eq!(Preference::System.mode(WindowAppearance::Dark), Mode::Dark);
        assert_eq!(Preference::Light.mode(WindowAppearance::Dark), Mode::Light);
    }

    #[test]
    fn short_and_long_hex_colors_parse() {
        assert_eq!(parse_hex("#fff"), Some(gpui::rgb(0xffffff)));
        assert_eq!(parse_hex("#0D0D0D"), Some(gpui::rgb(0x0d0d0d)));
        assert_eq!(parse_hex("0D0D0D"), None);
        assert_eq!(parse_hex("#12345"), None);
    }
}
