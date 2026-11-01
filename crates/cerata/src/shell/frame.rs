//! The window frame where the compositor leaves decorations to the client
//! (GNOME on Wayland offers no server-side ones). A transparent margin on
//! every free edge, which GPUI's client inset keeps out of the window
//! geometry, takes the resize edges and the shadow; inside it the surface
//! has rounded corners and a 1 px border. Tiled edges lose their margin,
//! their rounding and the shadow. A server-decorated window is its surface
//! alone.

use gpui::{
    AnyElement, Bounds, CursorStyle, Decorations, HitboxBehavior, InteractiveElement, IntoElement,
    MouseButton, ParentElement, Pixels, Point, ResizeEdge, Size, Stateful, Styled, Tiling, Window,
    canvas, div, point, prelude::FluentBuilder as _, px, size,
};

use crate::editor::theme;

/// Width of the transparent margin around a free edge.
pub const MARGIN: Pixels = px(10.0);
/// Radius of the surface's free corners.
const CORNER_RADIUS: Pixels = px(10.0);
/// The surface's border, inside which panels round concentrically.
const BORDER: Pixels = px(1.0);

/// The free (untiled) edges of a client-decorated window. A
/// server-decorated window has none: the compositor draws its frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Free {
    pub top: bool,
    pub bottom: bool,
    pub left: bool,
    pub right: bool,
}

impl Free {
    pub fn of(window: &Window) -> Self {
        match window.window_decorations() {
            Decorations::Client { tiling } => Self::from_tiling(tiling),
            Decorations::Server => Self::default(),
        }
    }

    fn from_tiling(tiling: Tiling) -> Self {
        Self {
            top: !tiling.top,
            bottom: !tiling.bottom,
            left: !tiling.left,
            right: !tiling.right,
        }
    }

    /// The margin left of the surface, in window pixels.
    pub fn left_margin(self) -> f32 {
        if self.left { f32::from(MARGIN) } else { 0.0 }
    }

    /// The margins left and right of the surface, in window pixels.
    pub fn horizontal_margins(self) -> f32 {
        self.left_margin() + if self.right { f32::from(MARGIN) } else { 0.0 }
    }

    /// A floating window: every edge free.
    fn floating(self) -> bool {
        self.top && self.bottom && self.left && self.right
    }
}

/// Rounds the left corners of a panel painted against the surface's left
/// edge, so its fill follows the surface's free corners.
pub fn round_left<E: Styled + gpui::prelude::FluentBuilder>(element: E, free: Free) -> E {
    let radius = CORNER_RADIUS - BORDER;
    element
        .when(free.top && free.left, |e| e.rounded_tl(radius))
        .when(free.bottom && free.left, |e| e.rounded_bl(radius))
}

/// Wraps the window's root element (`surface`, which paints the window
/// background) in the frame its decorations call for.
pub fn frame(surface: Stateful<gpui::Div>, window: &mut Window) -> AnyElement {
    let Decorations::Client { tiling } = window.window_decorations() else {
        window.set_client_inset(px(0.0));
        return surface.into_any_element();
    };
    window.set_client_inset(MARGIN);
    let free = Free::from_tiling(tiling);
    let surface = surface
        .border_color(theme::border())
        .when(free.top && free.left, |s| s.rounded_tl(CORNER_RADIUS))
        .when(free.top && free.right, |s| s.rounded_tr(CORNER_RADIUS))
        .when(free.bottom && free.left, |s| s.rounded_bl(CORNER_RADIUS))
        .when(free.bottom && free.right, |s| s.rounded_br(CORNER_RADIUS))
        .when(free.top, |s| s.border_t(BORDER))
        .when(free.bottom, |s| s.border_b(BORDER))
        .when(free.left, |s| s.border_l(BORDER))
        .when(free.right, |s| s.border_r(BORDER))
        .when(free.floating(), |s| {
            s.shadow(vec![
                gpui::BoxShadow::new(px(0.0), px(0.0), gpui::hsla(0.0, 0.0, 0.0, 0.4))
                    .blur_radius(MARGIN / 2.0),
            ])
        });
    div()
        .id("window-frame")
        .size_full()
        .when(free.top, |d| d.pt(MARGIN))
        .when(free.bottom, |d| d.pb(MARGIN))
        .when(free.left, |d| d.pl(MARGIN))
        .when(free.right, |d| d.pr(MARGIN))
        // Repaint near an edge, so the cursor follows it.
        .on_mouse_move(move |event, window, _| {
            let window_size = window.window_bounds().get_bounds().size;
            if resize_edge(event.position, MARGIN * 2.0, window_size, tiling).is_some() {
                window.refresh();
            }
        })
        .on_mouse_down(MouseButton::Left, move |event, window, _| {
            let window_size = window.window_bounds().get_bounds().size;
            if let Some(edge) = resize_edge(event.position, MARGIN, window_size, tiling) {
                window.start_window_resize(edge);
            }
        })
        .child(surface)
        .child(
            canvas(
                |_, window, _| {
                    window.insert_hitbox(
                        Bounds::new(
                            point(px(0.0), px(0.0)),
                            window.window_bounds().get_bounds().size,
                        ),
                        HitboxBehavior::Normal,
                    )
                },
                move |_, hitbox, window, _| {
                    let window_size = window.window_bounds().get_bounds().size;
                    let Some(edge) =
                        resize_edge(window.mouse_position(), MARGIN, window_size, tiling)
                    else {
                        return;
                    };
                    let cursor = match edge {
                        ResizeEdge::Top | ResizeEdge::Bottom => CursorStyle::ResizeUpDown,
                        ResizeEdge::Left | ResizeEdge::Right => CursorStyle::ResizeLeftRight,
                        ResizeEdge::TopLeft | ResizeEdge::BottomRight => {
                            CursorStyle::ResizeUpLeftDownRight
                        }
                        ResizeEdge::TopRight | ResizeEdge::BottomLeft => {
                            CursorStyle::ResizeUpRightDownLeft
                        }
                    };
                    window.set_cursor_style(cursor, &hitbox);
                },
            )
            .size_full()
            .absolute(),
        )
        .into_any_element()
}

/// The edge a press at `pos` resizes: within `border` of a free edge, or
/// within one and a half `border` of a free corner.
fn resize_edge(
    pos: Point<Pixels>,
    border: Pixels,
    window_size: Size<Pixels>,
    tiling: Tiling,
) -> Option<ResizeEdge> {
    let inner = Bounds::new(Point::default(), window_size).inset(border * 1.5);
    if inner.contains(&pos) {
        return None;
    }
    let corner = size(border * 1.5, border * 1.5);
    let right = window_size.width - corner.width;
    let bottom = window_size.height - corner.height;
    let in_corner = |x: Pixels, y: Pixels| Bounds::new(point(x, y), corner).contains(&pos);
    if !tiling.top && !tiling.left && in_corner(px(0.0), px(0.0)) {
        return Some(ResizeEdge::TopLeft);
    }
    if !tiling.top && !tiling.right && in_corner(right, px(0.0)) {
        return Some(ResizeEdge::TopRight);
    }
    if !tiling.bottom && !tiling.left && in_corner(px(0.0), bottom) {
        return Some(ResizeEdge::BottomLeft);
    }
    if !tiling.bottom && !tiling.right && in_corner(right, bottom) {
        return Some(ResizeEdge::BottomRight);
    }
    if !tiling.top && pos.y <= border {
        Some(ResizeEdge::Top)
    } else if !tiling.bottom && pos.y >= window_size.height - border {
        Some(ResizeEdge::Bottom)
    } else if !tiling.left && pos.x <= border {
        Some(ResizeEdge::Left)
    } else if !tiling.right && pos.x >= window_size.width - border {
        Some(ResizeEdge::Right)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WINDOW: Size<Pixels> = Size {
        width: px(800.0),
        height: px(600.0),
    };

    #[test]
    fn a_floating_window_frees_every_edge() {
        let free = Free::from_tiling(Tiling::default());
        assert!(free.floating());
        assert_eq!(free.left_margin(), 10.0);
        assert_eq!(free.horizontal_margins(), 20.0);
    }

    #[test]
    fn a_tiled_edge_loses_its_margin_and_the_shadow() {
        let free = Free::from_tiling(Tiling {
            left: true,
            ..Tiling::default()
        });
        assert!(!free.floating());
        assert_eq!(free.left_margin(), 0.0);
        assert_eq!(free.horizontal_margins(), 10.0);
        assert_eq!(Free::from_tiling(Tiling::tiled()), Free::default());
    }

    #[test]
    fn edges_and_corners_resize_only_where_free() {
        let free = Tiling::default();
        let edge =
            |x: f32, y: f32, tiling| resize_edge(point(px(x), px(y)), MARGIN, WINDOW, tiling);
        assert_eq!(edge(2.0, 2.0, free), Some(ResizeEdge::TopLeft));
        assert_eq!(edge(798.0, 598.0, free), Some(ResizeEdge::BottomRight));
        assert_eq!(edge(400.0, 3.0, free), Some(ResizeEdge::Top));
        assert_eq!(edge(797.0, 300.0, free), Some(ResizeEdge::Right));
        assert_eq!(edge(400.0, 300.0, free), None);
        assert_eq!(edge(400.0, 3.0, Tiling::tiled()), None);
    }
}
