//! The menu bar Windows and Linux draw at the top of the window: the
//! same menus as macOS's native bar, each opening a dropdown whose entries
//! dispatch their action from the focused element.

use gpui::{
    AnyElement, Context, InteractiveElement, IntoElement, MouseButton, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, div, point, px,
};

use super::WindowView;
use crate::app_menu::{self, Entry};
use crate::editor::theme;
use crate::menu::{Item, Menu};

pub const HEIGHT: f32 = 28.0;

impl WindowView {
    /// Opens the drawn menu at `index` below its title.
    pub fn open_app_menu(&mut self, index: usize, left: f32, cx: &mut Context<Self>) {
        let Some((_, entries)) = app_menu::menus().into_iter().nth(index) else {
            return;
        };
        let items = entries
            .into_iter()
            .map(|entry| match entry {
                Entry::Separator => Item::Separator,
                Entry::Item { label, action, .. } => {
                    Item::action(label, move |_: &mut WindowView, window, cx| {
                        window.dispatch_action(action.boxed_clone(), cx)
                    })
                }
            })
            .collect();
        self.tab_menu = Some(Menu::new(point(px(left), px(HEIGHT)), items));
        cx.notify();
    }
}

pub fn render(cx: &mut Context<WindowView>) -> AnyElement {
    let mut bar = div()
        .id("menu-bar")
        .role(gpui::Role::MenuBar)
        .flex_none()
        .h(px(HEIGHT))
        .w_full()
        .px(px(4.0))
        .flex()
        .items_center()
        .text_size(px(13.0))
        .border_b_1()
        .border_color(theme::rule());
    for (index, (name, _)) in app_menu::menus().into_iter().enumerate() {
        bar = bar.child(
            crate::a11y::control(
                div().id(SharedString::from(format!("menu-bar-{index}"))),
                gpui::Role::MenuItem,
                name,
            )
            .on_click(cx.listener(move |this, event: &gpui::ClickEvent, _, cx| {
                if let gpui::ClickEvent::Keyboard(click) = event {
                    let left = f32::from(click.bounds.origin.x);
                    this.open_app_menu(index, left, cx);
                }
            }))
            .h(px(22.0))
            .px(px(8.0))
            .flex()
            .items_center()
            .rounded(px(4.0))
            .cursor_pointer()
            .hover(|s| s.bg(theme::panel()))
            .child(name)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &gpui::MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    this.open_app_menu(index, f32::from(event.position.x) - 8.0, cx);
                }),
            ),
        );
    }
    bar.into_any_element()
}
