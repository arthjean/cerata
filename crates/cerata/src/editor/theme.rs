//! Editor colors (light theme). The theme system arrives with the app shell
//! (EP-004); until then these are Writer's light defaults.

use cerata_render::highlight::HighlightKind;
use gpui::{Hsla, rgb, rgba};

pub fn background() -> Hsla {
    rgb(0xffffff).into()
}
pub fn text() -> Hsla {
    rgb(0x1f2328).into()
}
pub fn muted() -> Hsla {
    rgb(0x8c959f).into()
}
pub fn accent() -> Hsla {
    rgb(0x0969da).into()
}
pub fn selection() -> Hsla {
    rgba(0x3b82f640).into()
}
pub fn code_background() -> Hsla {
    rgb(0xf3f4f6).into()
}
pub fn quote_bar() -> Hsla {
    rgb(0xd0d7de).into()
}
pub fn rule() -> Hsla {
    rgb(0xd8dee4).into()
}
pub fn scrollbar() -> Hsla {
    rgba(0x1f232833).into()
}
pub fn search_match() -> Hsla {
    rgba(0xfbbf2455).into()
}
pub fn search_current() -> Hsla {
    rgba(0xf59e0bb0).into()
}
pub fn panel() -> Hsla {
    rgb(0xf6f8fa).into()
}
pub fn border() -> Hsla {
    rgb(0xd0d7de).into()
}
pub fn danger() -> Hsla {
    rgb(0xcf222e).into()
}
pub fn transparent() -> Hsla {
    rgba(0x00000000).into()
}

/// Writer's `generalSyntaxHighlights` roles in light colors.
pub fn highlight(kind: HighlightKind) -> Hsla {
    use HighlightKind as K;
    let color = match kind {
        K::Link => 0x0969da,
        K::Keyword => 0xcf222e,
        K::Atom | K::Literal => 0x0550ae,
        K::String => 0x0a3069,
        K::Regexp => 0x116329,
        K::DefinitionVariable | K::DefinitionProperty => 0x8250df,
        K::LocalVariable => 0x1f2328,
        K::TypeName | K::ClassName => 0x953800,
        K::Macro => 0x6639ba,
        K::Comment => 0x6e7781,
        K::Invalid => 0xcf222e,
    };
    rgb(color).into()
}
