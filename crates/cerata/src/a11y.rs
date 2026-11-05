use gpui::{
    Div, InteractiveElement as _, KeyBinding, Role, SharedString, Stateful,
    StatefulInteractiveElement as _, Styled as _, actions,
};

use crate::editor::theme;

actions!(a11y, [FocusNext, FocusPrevious]);

pub fn bindings() -> Vec<KeyBinding> {
    vec![
        KeyBinding::new("tab", FocusNext, Some("!Editor")),
        KeyBinding::new("shift-tab", FocusPrevious, Some("!Editor")),
    ]
}

pub fn control(el: Stateful<Div>, role: Role, label: impl Into<SharedString>) -> Stateful<Div> {
    el.focusable()
        .tab_stop(true)
        .role(role)
        .aria_label(label)
        .border_1()
        .border_color(theme::transparent())
        .focus_visible(|s| s.border_color(theme::text()))
}

pub fn button(el: Stateful<Div>, label: impl Into<SharedString>) -> Stateful<Div> {
    control(el, Role::Button, label)
}
