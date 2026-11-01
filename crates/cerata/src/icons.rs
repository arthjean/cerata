//! Interface icons: the free Hugeicons Stroke Rounded set (MIT, see
//! `NOTICE`), copied from `@hugeicons/core-free-icons`
//! 4.4.0 and embedded in the binary.
//!
//! GPUI paints an SVG as a mask tinted by the element's own text color,
//! which an `svg()` does not inherit from its parent: every icon is built
//! with an explicit color.

use gpui::{Hsla, Pixels, Styled, Svg, px, svg};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    Add,
    ChevronDown,
    ChevronRight,
    ChevronUp,
    ArrowLeft,
    ArrowRight,
    Close,
    Check,
    Search,
    More,
    File,
}

impl Icon {
    /// The SVG source, also used as the rasterization cache key.
    pub fn data(self) -> &'static [u8] {
        match self {
            Icon::Add => include_bytes!("../assets/icons/add-01.svg"),
            Icon::ChevronDown => include_bytes!("../assets/icons/arrow-down-01.svg"),
            Icon::ChevronRight => include_bytes!("../assets/icons/arrow-right-01.svg"),
            Icon::ChevronUp => include_bytes!("../assets/icons/arrow-up-01.svg"),
            Icon::ArrowLeft => include_bytes!("../assets/icons/arrow-left-02.svg"),
            Icon::ArrowRight => include_bytes!("../assets/icons/arrow-right-02.svg"),
            Icon::Close => include_bytes!("../assets/icons/cancel-01.svg"),
            Icon::Check => include_bytes!("../assets/icons/tick-02.svg"),
            Icon::Search => include_bytes!("../assets/icons/search-01.svg"),
            Icon::More => include_bytes!("../assets/icons/more-horizontal.svg"),
            Icon::File => include_bytes!("../assets/icons/file-02.svg"),
        }
    }

    /// A stable cache key for `Window::paint_svg`.
    pub fn key(self) -> &'static str {
        match self {
            Icon::Add => "icons/add-01.svg",
            Icon::ChevronDown => "icons/arrow-down-01.svg",
            Icon::ChevronRight => "icons/arrow-right-01.svg",
            Icon::ChevronUp => "icons/arrow-up-01.svg",
            Icon::ArrowLeft => "icons/arrow-left-02.svg",
            Icon::ArrowRight => "icons/arrow-right-02.svg",
            Icon::Close => "icons/cancel-01.svg",
            Icon::Check => "icons/tick-02.svg",
            Icon::Search => "icons/search-01.svg",
            Icon::More => "icons/more-horizontal.svg",
            Icon::File => "icons/file-02.svg",
        }
    }
}

/// A square icon element of `size` pixels, tinted with `color`.
pub fn icon(icon: Icon, size: f32, color: Hsla) -> Svg {
    let size: Pixels = px(size);
    svg()
        .data(icon.data())
        .flex_none()
        .size(size)
        .text_color(color)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_icon_is_a_24px_svg() {
        let all = [
            Icon::Add,
            Icon::ChevronDown,
            Icon::ChevronRight,
            Icon::ChevronUp,
            Icon::ArrowLeft,
            Icon::ArrowRight,
            Icon::Close,
            Icon::Check,
            Icon::Search,
            Icon::More,
            Icon::File,
        ];
        for icon in all {
            let text = std::str::from_utf8(icon.data()).unwrap();
            assert!(
                text.starts_with("<svg") && text.contains("viewBox=\"0 0 24 24\""),
                "{icon:?}"
            );
            assert!(icon.key().ends_with(".svg"), "{icon:?}");
        }
    }
}
