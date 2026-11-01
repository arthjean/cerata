//! Keyboard and assistive-technology access for the shell's custom
//! controls: each is a Tab stop with a visible focus ring, announced with a
//! role and a name; Enter and Space click it (GPUI's keyboard click). Tab
//! and Shift+Tab move between them outside the editor.

use gpui::{
    Div, InteractiveElement as _, KeyBinding, Role, SharedString, Stateful,
    StatefulInteractiveElement as _, Styled as _, actions,
};

use crate::editor::theme;

actions!(a11y, [FocusNext, FocusPrevious]);

/// Tab and Shift+Tab in the window chrome. Outside the editor only: a
/// binding with no context outranks every context binding in GPUI's keymap
/// (`binding_enabled` gives it the deepest depth), which took Tab from the
/// editor's indentation. The editor moves the focus itself (Ctrl+M).
pub fn bindings() -> Vec<KeyBinding> {
    vec![
        KeyBinding::new("tab", FocusNext, Some("!Editor")),
        KeyBinding::new("shift-tab", FocusPrevious, Some("!Editor")),
    ]
}

/// Makes `el` a keyboard control named `label`. The focus ring is a 1 px
/// border in the text color (the accent misses 3:1 on light panels),
/// drawn only for keyboard focus.
pub fn control(el: Stateful<Div>, role: Role, label: impl Into<SharedString>) -> Stateful<Div> {
    el.focusable()
        .tab_stop(true)
        .role(role)
        .aria_label(label)
        .border_1()
        .border_color(theme::transparent())
        .focus_visible(|s| s.border_color(theme::text()))
}

/// [`control`] with the button role.
pub fn button(el: Stateful<Div>, label: impl Into<SharedString>) -> Stateful<Div> {
    control(el, Role::Button, label)
}
