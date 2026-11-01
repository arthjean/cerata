//! Editor and chrome colors. The app's theme (`crate::theme`) installs the
//! palette derived from the settings with [`set`]; until then (and in the
//! examples, which build this module alone) Writer's light defaults apply.

use std::cell::Cell;

use cerata_render::highlight::HighlightKind;
use gpui::{Hsla, rgb, rgba};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Palette {
    pub background: Hsla,
    pub text: Hsla,
    pub muted: Hsla,
    pub accent: Hsla,
    pub selection: Hsla,
    pub code_background: Hsla,
    pub quote_bar: Hsla,
    pub rule: Hsla,
    pub scrollbar: Hsla,
    pub panel: Hsla,
    pub border: Hsla,
    /// Opaque surface of what floats over text (menus, palette, pickers,
    /// notices, the find bar): a translucent one lets the text show through.
    pub overlay: Hsla,
    /// Error text and invalid borders, readable on the mode's panels.
    pub danger: Hsla,
    pub dark: bool,
}

impl Default for Palette {
    fn default() -> Self {
        Self {
            background: rgb(0xffffff).into(),
            text: rgb(0x1f2328).into(),
            muted: rgb(0x8c959f).into(),
            accent: rgb(0x0969da).into(),
            selection: rgba(0x3b82f640).into(),
            code_background: rgb(0xf3f4f6).into(),
            quote_bar: rgb(0xd0d7de).into(),
            rule: rgb(0xd8dee4).into(),
            scrollbar: rgba(0x1f232833).into(),
            panel: rgb(0xf6f8fa).into(),
            border: rgb(0xd0d7de).into(),
            overlay: rgb(0xffffff).into(),
            danger: rgb(0xa40e26).into(),
            dark: false,
        }
    }
}

thread_local! {
    /// The UI thread's palette (tests run one app per thread).
    static PALETTE: Cell<Option<Palette>> = const { Cell::new(None) };
}

/// Installs the palette every color function reads.
pub fn set(palette: Palette) {
    PALETTE.with(|current| current.set(Some(palette)));
}

pub fn palette() -> Palette {
    PALETTE.with(Cell::get).unwrap_or_default()
}

pub fn background() -> Hsla {
    palette().background
}
pub fn text() -> Hsla {
    palette().text
}
pub fn muted() -> Hsla {
    palette().muted
}
pub fn accent() -> Hsla {
    palette().accent
}
pub fn selection() -> Hsla {
    palette().selection
}
pub fn code_background() -> Hsla {
    palette().code_background
}
pub fn quote_bar() -> Hsla {
    palette().quote_bar
}
pub fn rule() -> Hsla {
    palette().rule
}
pub fn scrollbar() -> Hsla {
    palette().scrollbar
}
pub fn search_match() -> Hsla {
    rgba(0xfbbf2455).into()
}
pub fn search_current() -> Hsla {
    rgba(0xf59e0bb0).into()
}
pub fn panel() -> Hsla {
    palette().panel
}
pub fn border() -> Hsla {
    palette().border
}
pub fn danger() -> Hsla {
    palette().danger
}
pub fn overlay() -> Hsla {
    palette().overlay
}
pub fn transparent() -> Hsla {
    rgba(0x00000000).into()
}

/// Writer's `generalSyntaxHighlights` roles, in light or dark colors.
pub fn highlight(kind: HighlightKind) -> Hsla {
    use HighlightKind as K;
    let dark = palette().dark;
    let (light, night) = match kind {
        K::Link => (0x0969da, 0x4493f8),
        K::Keyword => (0xcf222e, 0xff7b72),
        K::Atom | K::Literal => (0x0550ae, 0x79c0ff),
        K::String => (0x0a3069, 0xa5d6ff),
        K::Regexp => (0x116329, 0x7ee787),
        K::DefinitionVariable | K::DefinitionProperty => (0x8250df, 0xd2a8ff),
        K::LocalVariable => (0x1f2328, 0xe6edf3),
        K::TypeName | K::ClassName => (0x953800, 0xffa657),
        K::Macro => (0x6639ba, 0xd2a8ff),
        K::Comment => (0x6e7781, 0x8b949e),
        K::Invalid => (0xcf222e, 0xff7b72),
    };
    rgb(if dark { night } else { light }).into()
}
