//! The Settings tab (`settings-panel/index.tsx:11-88`): one card per
//! category (Appearance, Typography, Light Theme, Dark Theme, then the
//! schema's order), one control per value type, and a Reset button on
//! every value that differs from its default. Writes go through the
//! window, which applies them.

use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use cerata_core::settings::{ConfigValue, SettingDef, ValueType};
use cerata_core::workspace::WindowState;
use gpui::{
    App, AppContext as _, Bounds, ClickEvent, Context, Entity, EventEmitter, FocusHandle,
    Focusable, FontWeight, InteractiveElement, IntoElement, KeyDownEvent, MouseButton,
    MouseDownEvent, MouseMoveEvent, ParentElement, Pixels, Render, Role, SharedString,
    StatefulInteractiveElement, Styled, Subscription, Toggled, Window, canvas, div,
    prelude::FluentBuilder as _, px,
};

use crate::editor::text_field::{FieldEvent, TextField};
use crate::editor::theme;
use crate::icons::{Icon, icon};
use crate::menu::{HasMenu, Item, Menu};
use crate::theme::{Mode, Preset};

const BEFORE_THEMES: [&str; 2] = ["Appearance", "Typography"];
const THEME_CATEGORY: &str = "Theme";
const PRIMARIES: [&str; 5] = [
    "accent",
    "background",
    "foreground",
    "translucent",
    "contrast",
];

/// What the panel asks the window to do.
#[derive(Clone, Debug, PartialEq)]
pub enum PanelEvent {
    Set(String, ConfigValue),
    Reset(String),
}

/// A card of the panel: its title and its keys, in order.
#[derive(Clone, Debug, PartialEq)]
pub struct Section {
    pub title: String,
    pub keys: Vec<String>,
}

/// The cards in Writer's order: Appearance and Typography, the two theme
/// cards (preset first, then the five primaries), then every other
/// category in schema order.
pub fn sections(definitions: &[SettingDef]) -> Vec<Section> {
    let mut categories: Vec<Section> = Vec::new();
    for def in definitions.iter().filter(|d| d.category != THEME_CATEGORY) {
        match categories.iter_mut().find(|s| s.title == def.category) {
            Some(section) => section.keys.push(def.key.clone()),
            None => categories.push(Section {
                title: def.category.clone(),
                keys: vec![def.key.clone()],
            }),
        }
    }
    let mut ordered: Vec<Section> = BEFORE_THEMES
        .iter()
        .filter_map(|title| categories.iter().find(|s| s.title == *title).cloned())
        .collect();
    for (mode, title) in [(Mode::Light, "Light Theme"), (Mode::Dark, "Dark Theme")] {
        let preset = format!("theme.{}.preset", mode.id());
        let keys: Vec<String> = std::iter::once(preset)
            .chain(PRIMARIES.iter().map(|p| format!("theme.{}.{p}", mode.id())))
            .filter(|key| definitions.iter().any(|d| d.key == *key))
            .collect();
        ordered.push(Section {
            title: title.into(),
            keys,
        });
    }
    ordered.extend(
        categories
            .into_iter()
            .filter(|s| !BEFORE_THEMES.contains(&s.title.as_str()) && s.title != HIDDEN_CATEGORY),
    );
    ordered
}

/// Telemetry was dropped from v1: its switch and address would promise a
/// sharing that never happens, so the panel leaves them out.
const HIDDEN_CATEGORY: &str = "Privacy";

/// Installed families for the font picker (`fonts.rs:34-45`): no macOS
/// internal names (leading dot), sorted ignoring case, no duplicates.
pub fn font_families(names: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut families: Vec<String> = names.into_iter().filter(|n| !n.starts_with('.')).collect();
    families.sort_by(|a, b| (a.to_lowercase(), a).cmp(&(b.to_lowercase(), b)));
    families.dedup();
    families
}

fn strip_quotes(name: &str) -> &str {
    for quote in ['"', '\''] {
        if name.len() >= 2 && name.starts_with(quote) && name.ends_with(quote) {
            return &name[1..name.len() - 1];
        }
    }
    name
}

/// First family of a stack, unquoted (`font-stack.ts:10-12`).
pub fn first_family(stack: &str) -> String {
    strip_quotes(stack.split(',').next().unwrap_or("").trim()).to_owned()
}

/// `stack` with its first family replaced by `family`; the tail stays,
/// without a duplicate of `family` (`font-stack.ts:17-26`).
pub fn stack_with_family(family: &str, stack: &str) -> String {
    let name = family.trim();
    let plain = name.chars().next().is_some_and(|c| c.is_ascii_alphabetic())
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
    let quoted = if plain {
        name.to_owned()
    } else {
        format!("\"{}\"", name.replace('"', "\\\""))
    };
    std::iter::once(quoted)
        .chain(
            stack
                .split(',')
                .skip(1)
                .map(str::trim)
                .filter(|part| !part.is_empty() && strip_quotes(part) != name)
                .map(str::to_owned),
        )
        .collect::<Vec<_>>()
        .join(", ")
}

/// A typed color is kept only as `#RGB` or `#RRGGBB`, upper-cased.
pub fn valid_color(text: &str) -> Option<String> {
    crate::theme::parse_hex(text.trim()).map(|_| text.trim().to_uppercase())
}

/// The value a slider at `fraction` of its track stands for.
pub fn slider_value(fraction: f32, min: f64, max: f64, step: f64) -> f64 {
    let raw = min + f64::from(fraction.clamp(0.0, 1.0)) * (max - min);
    let step = if step > 0.0 { step } else { 1.0 };
    (((raw - min) / step).round() * step + min).clamp(min, max)
}

fn display_number(value: f64) -> String {
    if value.fract() == 0.0 {
        format!("{value:.0}")
    } else {
        format!("{value}")
    }
}

fn text_of(value: Option<&ConfigValue>, def: &SettingDef) -> String {
    match (value, def.value_type) {
        (Some(ConfigValue::Number(n)), _) => display_number(*n),
        (Some(ConfigValue::String(s)), _) => s.clone(),
        _ => String::new(),
    }
}

pub struct SettingsPanel {
    state: Arc<WindowState>,
    fonts: Vec<String>,
    presets: Vec<Preset>,
    pub fields: HashMap<String, Entity<TextField>>,
    lists: HashMap<String, Vec<Entity<TextField>>>,
    sliders: HashMap<String, Rc<Cell<Bounds<Pixels>>>>,
    /// The slider being dragged and its value, written on release.
    dragging: Option<(String, f64)>,
    pub menu: Option<Menu<Self>>,
    seen_revision: Option<u64>,
    focus_handle: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<PanelEvent> for SettingsPanel {}

impl Focusable for SettingsPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl HasMenu for SettingsPanel {
    fn menu(&mut self) -> &mut Option<Menu<Self>> {
        &mut self.menu
    }
}

impl SettingsPanel {
    pub fn new(state: Arc<WindowState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut panel = Self {
            state,
            fonts: font_families(window.text_system().all_font_names()),
            presets: crate::theme::presets(),
            fields: HashMap::new(),
            lists: HashMap::new(),
            sliders: HashMap::new(),
            dragging: None,
            menu: None,
            seen_revision: None,
            focus_handle: cx.focus_handle(),
            _subscriptions: Vec::new(),
        };
        let definitions: Vec<SettingDef> = panel.state.settings().schema().definitions().to_vec();
        for def in definitions {
            match def.value_type {
                ValueType::String | ValueType::Number | ValueType::Color
                    if !def.key.ends_with(".preset") =>
                {
                    let field = panel.text_field(&def, window, cx);
                    panel.fields.insert(def.key.clone(), field);
                }
                _ => {}
            }
        }
        panel.sync_fields(cx);
        panel
    }

    fn value(&self, key: &str) -> Option<ConfigValue> {
        self.state.settings().get(key)
    }

    fn def(&self, key: &str) -> Option<SettingDef> {
        self.state.settings().schema().get(key).cloned()
    }

    pub fn is_modified(&self, key: &str) -> bool {
        self.def(key)
            .is_some_and(|def| self.value(key).as_ref() != Some(&def.default))
    }

    fn text_field(
        &mut self,
        def: &SettingDef,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<TextField> {
        let placeholder = def.placeholder.clone().unwrap_or_default();
        let field = cx.new(|cx| TextField::new(placeholder, cx));
        let key = def.key.clone();
        self._subscriptions.push(cx.subscribe_in(&field, window, {
            let key = key.clone();
            move |this, field, event: &FieldEvent, _, cx| match event {
                FieldEvent::Submit { .. } => this.commit_field(&key, cx),
                FieldEvent::Changed => field.update(cx, |field, cx| {
                    field.invalid = false;
                    cx.notify();
                }),
                FieldEvent::Escape => {}
            }
        }));
        let focus = field.read(cx).focus_handle.clone();
        self._subscriptions
            .push(cx.on_blur(&focus, window, move |this, _, cx| {
                this.commit_field(&key, cx)
            }));
        field
    }

    /// Fields that are not being edited show the current values.
    fn sync_fields(&mut self, cx: &mut Context<Self>) {
        let revision = self.state.settings().revision();
        if self.seen_revision == Some(revision) {
            return;
        }
        self.seen_revision = Some(revision);
        for (key, field) in self.fields.clone() {
            let Some(def) = self.def(&key) else { continue };
            let text = text_of(self.value(&key).as_ref(), &def);
            field.update(cx, |field, cx| {
                if !field.invalid && field.text != text {
                    field.set_text(&text, cx);
                }
            });
        }
    }

    /// Enter or a lost focus: a valid value is written, an invalid color or
    /// number turns red and is not.
    pub fn commit_field(&mut self, key: &str, cx: &mut Context<Self>) {
        let (Some(field), Some(def)) = (self.fields.get(key).cloned(), self.def(key)) else {
            return;
        };
        let text = field.read(cx).text.clone();
        let value = match def.value_type {
            ValueType::Color => valid_color(&text).map(ConfigValue::String),
            ValueType::Number => text
                .trim()
                .parse::<f64>()
                .ok()
                .filter(|n| n.is_finite())
                .map(ConfigValue::Number),
            _ => Some(ConfigValue::String(text.clone())),
        };
        match value {
            Some(value) => {
                field.update(cx, |field, cx| {
                    field.invalid = false;
                    cx.notify();
                });
                if self.value(key).as_ref() != Some(&value) {
                    cx.emit(PanelEvent::Set(key.to_owned(), value));
                }
            }
            None => field.update(cx, |field, cx| {
                field.invalid = true;
                cx.notify();
            }),
        }
    }

    fn set(&mut self, key: &str, value: ConfigValue, cx: &mut Context<Self>) {
        cx.emit(PanelEvent::Set(key.to_owned(), value));
        cx.notify();
    }

    pub fn reset(&mut self, key: &str, cx: &mut Context<Self>) {
        if let Some(field) = self.fields.get(key) {
            field.update(cx, |field, _| field.invalid = false);
        }
        self.lists.remove(key);
        self.seen_revision = None;
        cx.emit(PanelEvent::Reset(key.to_owned()));
        cx.notify();
    }

    /// Choosing a preset writes its name and its five values for that mode.
    pub fn apply_preset(&mut self, mode: Mode, name: &str, cx: &mut Context<Self>) {
        let Some(preset) = self.presets.iter().find(|p| p.name == name).cloned() else {
            return;
        };
        let values = match mode {
            Mode::Light => preset.light,
            Mode::Dark => preset.dark,
        };
        let key = |suffix: &str| format!("theme.{}.{suffix}", mode.id());
        self.set(&key("preset"), ConfigValue::String(preset.name.clone()), cx);
        self.set(&key("accent"), ConfigValue::String(values.accent), cx);
        self.set(
            &key("background"),
            ConfigValue::String(values.background),
            cx,
        );
        self.set(
            &key("foreground"),
            ConfigValue::String(values.foreground),
            cx,
        );
        self.set(
            &key("translucent"),
            ConfigValue::Number(values.translucent),
            cx,
        );
        self.set(&key("contrast"), ConfigValue::Number(values.contrast), cx);
        self.seen_revision = None;
    }

    /// Picking a font replaces the first family of the stack only.
    pub fn pick_font(&mut self, key: &str, family: &str, cx: &mut Context<Self>) {
        let current = self
            .value(key)
            .and_then(|v| v.as_str().map(str::to_owned))
            .unwrap_or_default();
        self.set(
            key,
            ConfigValue::String(stack_with_family(family, &current)),
            cx,
        );
    }

    fn list_fields(
        &mut self,
        key: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<Entity<TextField>> {
        if let Some(fields) = self.lists.get(key) {
            return fields.clone();
        }
        let items: Vec<String> = self
            .value(key)
            .and_then(|v| v.as_list().map(<[String]>::to_vec))
            .unwrap_or_default();
        let fields: Vec<Entity<TextField>> = items
            .iter()
            .map(|item| self.list_item(key, item, window, cx))
            .collect();
        self.lists.insert(key.to_owned(), fields.clone());
        fields
    }

    fn list_item(
        &mut self,
        key: &str,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<TextField> {
        let field = cx.new(|cx| {
            let mut field = TextField::new("", cx);
            field.set_text(text, cx);
            field
        });
        let commit = key.to_owned();
        self._subscriptions.push(cx.subscribe_in(
            &field,
            window,
            move |this, _, event: &FieldEvent, _, cx| {
                if let FieldEvent::Submit { .. } = event {
                    this.commit_list(&commit, cx);
                }
            },
        ));
        let focus = field.read(cx).focus_handle.clone();
        let commit = key.to_owned();
        self._subscriptions
            .push(cx.on_blur(&focus, window, move |this, _, cx| {
                this.commit_list(&commit, cx)
            }));
        field
    }

    /// The list's non-empty items, written when they changed.
    pub fn commit_list(&mut self, key: &str, cx: &mut Context<Self>) {
        let Some(fields) = self.lists.get(key) else {
            return;
        };
        let items: Vec<String> = fields
            .iter()
            .map(|field| field.read(cx).text.trim().to_owned())
            .filter(|item| !item.is_empty())
            .collect();
        let value = ConfigValue::List(items);
        if self.value(key).as_ref() != Some(&value) {
            self.set(key, value, cx);
        }
    }

    fn add_list_item(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.list_fields(key, window, cx);
        let field = self.list_item(key, "", window, cx);
        let focus = field.read(cx).focus_handle.clone();
        if let Some(fields) = self.lists.get_mut(key) {
            fields.push(field);
        }
        window.focus(&focus, cx);
        cx.notify();
    }

    fn remove_list_item(&mut self, key: &str, index: usize, cx: &mut Context<Self>) {
        if let Some(fields) = self.lists.get_mut(key)
            && index < fields.len()
        {
            fields.remove(index);
        }
        self.commit_list(key, cx);
        cx.notify();
    }

    fn open_select(&mut self, key: &str, position: gpui::Point<Pixels>, cx: &mut Context<Self>) {
        let Some(def) = self.def(key) else { return };
        let current = self
            .value(key)
            .and_then(|v| v.as_str().map(str::to_owned))
            .unwrap_or_default();
        let items: Vec<Item<Self>> = if let Some(mode) = preset_mode(key) {
            self.presets
                .iter()
                .map(|preset| {
                    let name = preset.name.clone();
                    Item::check(
                        preset.name.clone(),
                        preset.name == current,
                        move |this: &mut Self, _, cx| this.apply_preset(mode, &name, cx),
                    )
                })
                .collect()
        } else if def.value_type == ValueType::Font {
            let selected = first_family(&current);
            let mut families = self.fonts.clone();
            if !selected.is_empty() && !families.contains(&selected) {
                families.insert(0, selected.clone());
            }
            families
                .into_iter()
                .map(|family| {
                    let key = key.to_owned();
                    let checked = family == selected;
                    Item::check(family.clone(), checked, move |this: &mut Self, _, cx| {
                        this.pick_font(&key, &family, cx)
                    })
                })
                .collect()
        } else {
            def.options
                .clone()
                .unwrap_or_default()
                .into_iter()
                .map(|option| {
                    let key = key.to_owned();
                    let checked = option == current;
                    Item::check(option.clone(), checked, move |this: &mut Self, _, cx| {
                        this.set(&key, ConfigValue::String(option.clone()), cx)
                    })
                })
                .collect()
        };
        self.menu = Some(Menu::new(position, items));
        cx.notify();
    }

    fn slider_at(&mut self, key: &str, x: Pixels) -> Option<f64> {
        let def = self.def(key)?;
        let bounds = self.sliders.get(key)?.get();
        let width = f32::from(bounds.size.width);
        if width <= 0.0 {
            return None;
        }
        let fraction = (f32::from(x) - f32::from(bounds.origin.x)) / width;
        Some(slider_value(
            fraction,
            def.min.unwrap_or(0.0),
            def.max.unwrap_or(100.0),
            def.step.unwrap_or(1.0),
        ))
    }

    fn end_drag(&mut self, cx: &mut Context<Self>) {
        if let Some((key, value)) = self.dragging.take() {
            if self.value(&key).and_then(|v| v.as_f64()) != Some(value) {
                self.set(&key, ConfigValue::Number(value), cx);
            }
            cx.notify();
        }
    }
}

fn preset_mode(key: &str) -> Option<Mode> {
    match key {
        "theme.light.preset" => Some(Mode::Light),
        "theme.dark.preset" => Some(Mode::Dark),
        _ => None,
    }
}

// ---- Rendering ----

fn input_box() -> gpui::Div {
    div()
        .h(px(36.0))
        .px(px(12.0))
        .flex()
        .items_center()
        .rounded(px(8.0))
        .bg(theme::panel())
        .text_size(px(13.0))
}

impl SettingsPanel {
    fn control(
        &mut self,
        def: &SettingDef,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let key = def.key.clone();
        let value = self.value(&key);
        let id = |suffix: &str| SharedString::from(format!("setting-{key}-{suffix}"));
        let name = def.label.clone();
        if preset_mode(&key).is_some()
            || matches!(def.value_type, ValueType::Enum | ValueType::Font)
        {
            let shown = match (def.value_type, value.as_ref().and_then(|v| v.as_str())) {
                (ValueType::Font, Some(stack)) => first_family(stack),
                (_, Some(text)) => text.to_owned(),
                _ => String::new(),
            };
            let select_key = key.clone();
            let keyboard_key = key.clone();
            return crate::a11y::control(input_box().id(id("select")), Role::ComboBox, name)
                .aria_expanded(false)
                .on_click(cx.listener(move |this, event: &ClickEvent, _, cx| {
                    if let ClickEvent::Keyboard(click) = event {
                        this.open_select(&keyboard_key, click.bounds.bottom_left(), cx);
                    }
                }))
                .w(px(if def.value_type == ValueType::Font {
                    256.0
                } else {
                    176.0
                }))
                .justify_between()
                .cursor_pointer()
                .hover(|s| s.bg(theme::border()))
                .child(
                    div()
                        .overflow_hidden()
                        .text_ellipsis()
                        .whitespace_nowrap()
                        .child(shown),
                )
                .child(icon(Icon::ChevronDown, 12.0, theme::muted()))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                        cx.stop_propagation();
                        this.open_select(&select_key, event.position, cx);
                    }),
                )
                .into_any_element();
        }
        match def.value_type {
            ValueType::Boolean => {
                let on = value
                    .as_ref()
                    .and_then(ConfigValue::as_bool)
                    .unwrap_or(false);
                crate::a11y::control(div().id(id("switch")), Role::Switch, name)
                    .aria_toggled(if on { Toggled::True } else { Toggled::False })
                    .w(px(36.0))
                    .h(px(20.0))
                    .p(px(2.0))
                    .rounded(px(10.0))
                    .cursor_pointer()
                    .bg(if on { theme::accent() } else { theme::border() })
                    .flex()
                    .when(on, |d| d.justify_end())
                    .child(div().size(px(16.0)).rounded(px(8.0)).bg(gpui::white()))
                    .on_click(
                        cx.listener(move |this, _, _, cx| {
                            this.set(&key, ConfigValue::Bool(!on), cx)
                        }),
                    )
                    .into_any_element()
            }
            ValueType::Range => {
                let min = def.min.unwrap_or(0.0);
                let max = def.max.unwrap_or(100.0);
                let current = match &self.dragging {
                    Some((k, v)) if *k == key => *v,
                    _ => value.as_ref().and_then(ConfigValue::as_f64).unwrap_or(min),
                };
                let fraction = if max > min {
                    ((current - min) / (max - min)) as f32
                } else {
                    0.0
                };
                let cell = self.sliders.entry(key.clone()).or_default().clone();
                let down_key = key.clone();
                let step_key = key.clone();
                let step = def.step.unwrap_or(1.0);
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.0))
                    .child(
                        crate::a11y::control(div().id(id("slider")), Role::Slider, name)
                            .aria_numeric_value(current)
                            .on_key_down(cx.listener(move |this, event: &KeyDownEvent, _, cx| {
                                let next = match event.keystroke.key.as_str() {
                                    "left" | "down" => current - step,
                                    "right" | "up" => current + step,
                                    "home" => min,
                                    "end" => max,
                                    _ => return,
                                };
                                cx.stop_propagation();
                                let next = next.clamp(min, max);
                                if (next - current).abs() > f64::EPSILON {
                                    this.set(&step_key, ConfigValue::Number(next), cx);
                                }
                            }))
                            .relative()
                            .w(px(160.0))
                            .h(px(20.0))
                            .cursor_pointer()
                            .flex()
                            .items_center()
                            .child(
                                canvas(move |bounds, _, _| cell.set(bounds), |_, _, _, _| {})
                                    .absolute()
                                    .size_full(),
                            )
                            .child(
                                div()
                                    .w_full()
                                    .h(px(4.0))
                                    .rounded(px(2.0))
                                    .bg(theme::border())
                                    .child(
                                        div()
                                            .h_full()
                                            .rounded(px(2.0))
                                            .bg(theme::accent())
                                            .w(gpui::relative(fraction.clamp(0.0, 1.0))),
                                    ),
                            )
                            .child(
                                div()
                                    .absolute()
                                    .left(gpui::relative(fraction.clamp(0.0, 1.0)))
                                    .ml(px(-7.0))
                                    .size(px(14.0))
                                    .rounded(px(7.0))
                                    .bg(gpui::white())
                                    .border_1()
                                    .border_color(theme::border()),
                            )
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                                    if let Some(value) = this.slider_at(&down_key, event.position.x)
                                    {
                                        this.dragging = Some((down_key.clone(), value));
                                        cx.notify();
                                    }
                                }),
                            ),
                    )
                    .child(
                        div()
                            .w(px(36.0))
                            .text_size(px(13.0))
                            .text_color(theme::muted())
                            .child(display_number(current)),
                    )
                    .into_any_element()
            }
            ValueType::List => {
                let fields = self.list_fields(&key, window, cx);
                let mut column = div().flex().flex_col().items_end().gap(px(6.0));
                for (index, field) in fields.into_iter().enumerate() {
                    let remove_key = key.clone();
                    column = column.child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(4.0))
                            .child(input_box().w(px(256.0)).child(field))
                            .child(
                                crate::a11y::button(
                                    div().id(SharedString::from(format!(
                                        "setting-{key}-remove-{index}"
                                    ))),
                                    format!("Remove item {}", index + 1),
                                )
                                .size(px(20.0))
                                .flex()
                                .items_center()
                                .justify_center()
                                .cursor_pointer()
                                .group("setting-remove")
                                .child(
                                    icon(Icon::Close, 14.0, theme::muted())
                                        .group_hover("setting-remove", |s| {
                                            s.text_color(theme::text())
                                        }),
                                )
                                .on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        this.remove_list_item(&remove_key, index, cx)
                                    },
                                )),
                            ),
                    );
                }
                let add_key = key.clone();
                column
                    .child(
                        crate::a11y::button(div().id(id("add")), format!("Add to {name}"))
                            .mr(px(24.0))
                            .w(px(256.0))
                            .h(px(36.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(8.0))
                            .border_1()
                            .border_dashed()
                            .border_color(theme::border())
                            .text_size(px(13.0))
                            .text_color(theme::muted())
                            .cursor_pointer()
                            .hover(|s| s.text_color(theme::text()))
                            .child("+ Add")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.add_list_item(&add_key, window, cx)
                            })),
                    )
                    .into_any_element()
            }
            ValueType::Color => {
                let Some(field) = self.fields.get(&key).cloned() else {
                    return div().into_any_element();
                };
                let invalid = field.read(cx).invalid;
                let swatch = value
                    .as_ref()
                    .and_then(|v| v.as_str())
                    .and_then(crate::theme::parse_hex)
                    .unwrap_or(gpui::rgb(0x000000));
                input_box()
                    .w(px(176.0))
                    .pl(px(4.0))
                    .gap(px(8.0))
                    .border_1()
                    .border_color(if invalid {
                        theme::danger()
                    } else {
                        theme::transparent()
                    })
                    .child(
                        div()
                            .size(px(28.0))
                            .rounded(px(6.0))
                            .border_1()
                            .border_color(theme::border())
                            .bg(swatch),
                    )
                    .child(
                        div()
                            .flex_1()
                            .when(invalid, |d| d.text_color(theme::danger()))
                            .child(field),
                    )
                    .into_any_element()
            }
            _ => {
                let Some(field) = self.fields.get(&key).cloned() else {
                    return div().into_any_element();
                };
                let invalid = field.read(cx).invalid;
                input_box()
                    .w(px(if def.value_type == ValueType::Number {
                        96.0
                    } else {
                        256.0
                    }))
                    .border_1()
                    .border_color(if invalid {
                        theme::danger()
                    } else {
                        theme::transparent()
                    })
                    .child(div().flex_1().child(field))
                    .into_any_element()
            }
        }
    }

    fn row(
        &mut self,
        def: &SettingDef,
        first: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let modified = self.is_modified(&def.key);
        let key = def.key.clone();
        let control = self.control(def, window, cx);
        let label = if preset_mode(&def.key).is_some() {
            "Preset".to_owned()
        } else {
            def.label.clone()
        };
        let label_name = label.clone();
        div()
            .flex()
            .items_start()
            .justify_between()
            .gap(px(16.0))
            .px(px(16.0))
            .py(px(14.0))
            .when(!first, |d| d.border_t_1().border_color(theme::rule()))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .text_size(px(13.0))
                            .font_weight(FontWeight::MEDIUM)
                            .child(label),
                    )
                    .when(!def.description.is_empty(), |d| {
                        d.child(
                            div()
                                .mt(px(2.0))
                                .text_size(px(13.0))
                                .text_color(theme::muted())
                                .child(def.description.clone()),
                        )
                    }),
            )
            .child(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .when(modified, |d| {
                        d.child(
                            crate::a11y::button(
                                div().id(SharedString::from(format!("setting-{key}-reset"))),
                                format!("Reset {label_name} to its default"),
                            )
                            .px(px(8.0))
                            .py(px(2.0))
                            .rounded(px(10.0))
                            .text_size(px(11.0))
                            .text_color(theme::muted())
                            .cursor_pointer()
                            .hover(|s| s.bg(theme::panel()).text_color(theme::text()))
                            .child("Reset")
                            .on_click(cx.listener(move |this, _, _, cx| this.reset(&key, cx))),
                        )
                    })
                    .child(control),
            )
            .into_any_element()
    }
}

impl Render for SettingsPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_fields(cx);
        let definitions: Vec<SettingDef> = self.state.settings().schema().definitions().to_vec();
        let mut content = div()
            .mx_auto()
            .w_full()
            .max_w(px(672.0))
            .px(px(32.0))
            .pt(px(128.0))
            .pb(px(96.0))
            .child(
                div()
                    .mb(px(40.0))
                    .text_size(px(24.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Preferences"),
            );
        for section in sections(&definitions) {
            let mut card = div()
                .mx(px(-16.0))
                .rounded(px(16.0))
                .border_1()
                .border_color(theme::rule())
                .bg(theme::code_background());
            for (ix, key) in section.keys.iter().enumerate() {
                if let Some(def) = definitions.iter().find(|d| d.key == *key) {
                    card = card.child(self.row(def, ix == 0, window, cx));
                }
            }
            content = content.child(
                div()
                    .mb(px(40.0))
                    .child(
                        div()
                            .mb(px(12.0))
                            .text_size(px(13.0))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme::muted())
                            .child(section.title.clone()),
                    )
                    .child(card),
            );
        }
        let mut root = div()
            .id("settings-panel")
            .track_focus(&self.focus_handle)
            .size_full()
            .overflow_y_scroll()
            .text_color(theme::text())
            .child(content);
        if self.dragging.is_some() {
            root = root
                .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                    if let Some((key, _)) = this.dragging.clone()
                        && let Some(value) = this.slider_at(&key, event.position.x)
                    {
                        this.dragging = Some((key, value));
                        cx.notify();
                    }
                }))
                .on_mouse_up(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| this.end_drag(cx)),
                )
                .on_mouse_up_out(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| this.end_drag(cx)),
                );
        }
        if let Some(menu) = self.menu.as_mut() {
            root = root.child(crate::menu::render(menu, window, cx));
        }
        root
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MONO_DEFAULT: &str = r#"ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, "Liberation Mono", "Courier New", monospace"#;

    // Ported from Writer `tests/font-stack.test.ts`.

    #[test]
    fn first_family_is_the_unquoted_head_of_the_stack() {
        assert_eq!(first_family(MONO_DEFAULT), "ui-monospace");
        assert_eq!(
            first_family(r#""JetBrains Mono", monospace"#),
            "JetBrains Mono"
        );
        assert_eq!(first_family("'SF Mono', Menlo"), "SF Mono");
        assert_eq!(first_family("Menlo"), "Menlo");
        assert_eq!(first_family(""), "");
    }

    #[test]
    fn stack_with_family_replaces_the_head_and_keeps_the_tail() {
        assert_eq!(
            stack_with_family("Menlo", "ui-monospace, monospace"),
            "Menlo, monospace"
        );
        assert_eq!(
            stack_with_family("Menlo", r#"Custom, Georgia, "Times New Roman", serif"#),
            r#"Menlo, Georgia, "Times New Roman", serif"#
        );
        assert_eq!(
            stack_with_family("JetBrains Mono", "Old, monospace"),
            r#""JetBrains Mono", monospace"#
        );
        assert_eq!(
            stack_with_family("Menlo", MONO_DEFAULT),
            r#"Menlo, SFMono-Regular, Monaco, Consolas, "Liberation Mono", "Courier New", monospace"#
        );
        assert_eq!(
            stack_with_family("Courier New", r#"Menlo, "Courier New", monospace"#),
            r#""Courier New", monospace"#
        );
        let once = stack_with_family("Fira Code", MONO_DEFAULT);
        assert_eq!(
            stack_with_family("JetBrains Mono", &once),
            r#""JetBrains Mono", SFMono-Regular, Menlo, Monaco, Consolas, "Liberation Mono", "Courier New", monospace"#
        );
        assert_eq!(stack_with_family("Menlo", "Old"), "Menlo");
        assert_eq!(stack_with_family("Menlo", ""), "Menlo");
    }

    #[test]
    fn font_families_hide_internal_names_sort_and_dedupe() {
        // Writer `fonts.rs:52-63`.
        assert_eq!(
            font_families(["Menlo", ".SF NS", "Arial", "menlo", "Menlo"].map(String::from)),
            ["Arial", "Menlo", "menlo"]
        );
    }

    #[test]
    fn sections_follow_writers_order() {
        let schema = cerata_core::settings::Schema::embedded().unwrap();
        let titles: Vec<String> = sections(schema.definitions())
            .into_iter()
            .map(|s| s.title)
            .collect();
        assert_eq!(
            titles,
            [
                "Appearance",
                "Typography",
                "Light Theme",
                "Dark Theme",
                "Editor",
                "Status Bar",
                "Files",
                "Workspace",
                "Window",
                "Search"
            ]
        );
        let light = &sections(schema.definitions())[2];
        assert_eq!(
            light.keys,
            [
                "theme.light.preset",
                "theme.light.accent",
                "theme.light.background",
                "theme.light.foreground",
                "theme.light.translucent",
                "theme.light.contrast"
            ]
        );
    }

    #[test]
    fn colors_and_sliders_validate() {
        assert_eq!(valid_color("#ff6a00"), Some("#FF6A00".into()));
        assert_eq!(valid_color("#abc"), Some("#ABC".into()));
        assert_eq!(valid_color("ff6a00"), None);
        assert_eq!(valid_color("#ff6a0"), None);
        assert_eq!(slider_value(0.5, 0.0, 100.0, 1.0), 50.0);
        assert_eq!(slider_value(1.5, 0.0, 100.0, 1.0), 100.0);
        assert_eq!(slider_value(0.333, 50.0, 300.0, 10.0), 130.0);
    }
}
